// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Files panel: the open sessions on top (the one in front first,
//! then those set aside), then every open audio file.
//!
//! * Click picks a file; Ctrl+click adds or removes one; Shift+click picks
//!   the run from the last one picked. Double-click opens it in the
//!   Waveform editor.
//! * Drag the picked files onto the Multitrack: one per track from the
//!   drop time, or with Alt one after another on one track.
//! * Columns: Name, Status, Duration, and Sample Rate and Channels when
//!   the panel is wide. Click a heading to sort, again to reverse, a third
//!   time for the order they were opened. The filter box narrows the list
//!   by name.
//! * A session row: double-click brings it to the front, × closes it
//!   (asking to save first), right-click for Save, Close and Close All.

use egui::{pos2, vec2, Align2, FontId, Rect, RichText, Rounding, Sense, Stroke};

use crate::sessions::Layout;
use crate::state::{DocId, Mode, State};
use crate::theme::{c, ca};
use af27_core::time::format_time;

/// What a drag from the Files panel carries: the files, in list order.
#[derive(Debug, Clone)]
pub struct DragFiles(pub Vec<DocId>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Name,
    Status,
    Duration,
    Rate,
    Channels,
}

#[derive(Debug, Default)]
pub struct FilesUi {
    /// Picked files, in the order they were picked.
    pub selected: Vec<DocId>,
    /// Where a Shift+click run starts.
    pub anchor: Option<DocId>,
    /// The sort column and whether it is reversed; None: opening order.
    pub sort: Option<(Col, bool)>,
    pub filter: String,
}

impl FilesUi {
    /// A file has closed.
    pub fn forget(&mut self, id: DocId) {
        self.selected.retain(|&d| d != id);
        if self.anchor == Some(id) {
            self.anchor = None;
        }
    }

    /// The picked files in the order the list shows them.
    pub fn picked_in(&self, order: &[DocId]) -> Vec<DocId> {
        order
            .iter()
            .copied()
            .filter(|d| self.selected.contains(d))
            .collect()
    }

    /// A click on `id`, with Ctrl (`toggle`) or Shift (`run`).
    pub fn click(&mut self, order: &[DocId], id: DocId, toggle: bool, run: bool) {
        if run {
            if let Some(a) = self.anchor {
                self.selected = range_select(order, a, id);
                return;
            }
        }
        if toggle {
            if let Some(i) = self.selected.iter().position(|&d| d == id) {
                self.selected.remove(i);
            } else {
                self.selected.push(id);
            }
        } else {
            self.selected = vec![id];
        }
        self.anchor = Some(id);
    }
}

/// A heading click: ascending, then reversed, then back to opening order.
pub fn next_sort(cur: Option<(Col, bool)>, col: Col) -> Option<(Col, bool)> {
    match cur {
        Some((c, false)) if c == col => Some((col, true)),
        Some((c, true)) if c == col => None,
        _ => Some((col, false)),
    }
}

/// Every file from `a` to `b` (either way round) in `order`.
pub fn range_select(order: &[DocId], a: DocId, b: DocId) -> Vec<DocId> {
    let (Some(i), Some(j)) = (
        order.iter().position(|&d| d == a),
        order.iter().position(|&d| d == b),
    ) else {
        return vec![b];
    };
    let (i, j) = (i.min(j), i.max(j));
    order[i..=j].to_vec()
}

struct Row {
    id: DocId,
    name: String,
    dirty: bool,
    dur: f64,
    rate: u32,
    ch: usize,
    used: bool,
}

const ROW_H: f32 = 20.0;

pub fn files(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("New Session")
            .on_hover_text("New Multitrack session (the one open is set aside, not closed)")
            .clicked()
        {
            st.new_session(None);
        }
        if ui
            .button("New Audio File")
            .on_hover_text("New empty audio file, listed here")
            .clicked()
        {
            st.new_file(48_000, 2);
        }
        if ui
            .button("Open")
            .on_hover_text("Open audio or a session (Ctrl+O)")
            .clicked()
        {
            crate::ui::open_dialog(st);
        }
    });
    ui.add(
        egui::TextEdit::singleline(&mut st.files_ui.filter)
            .hint_text("Filter by name")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(2.0);
    sessions(ui, st);

    // The files, filtered and sorted.
    let filter = st.files_ui.filter.to_lowercase();
    let mut rows: Vec<Row> = st
        .docs
        .iter()
        .filter(|d| filter.is_empty() || d.name.to_lowercase().contains(&filter))
        .map(|d| Row {
            id: d.id,
            name: d.name.clone(),
            dirty: d.dirty,
            dur: d.duration(),
            rate: d.buf.rate,
            ch: d.buf.num_channels(),
            used: false,
        })
        .collect();
    for r in &mut rows {
        r.used = st.doc_used(r.id);
    }
    if let Some((col, rev)) = st.files_ui.sort {
        rows.sort_by(|a, b| {
            let o = match col {
                Col::Name => crate::sessions::natural_cmp(&a.name, &b.name),
                Col::Status => status(a).cmp(status(b)),
                Col::Duration => a.dur.total_cmp(&b.dur),
                Col::Rate => a.rate.cmp(&b.rate),
                Col::Channels => a.ch.cmp(&b.ch),
            };
            if rev {
                o.reverse()
            } else {
                o
            }
        });
    }
    let order: Vec<DocId> = rows.iter().map(|r| r.id).collect();

    // Headings.
    let w = ui.available_width();
    let wide = w > 360.0;
    let cols = columns(w, wide);
    let (head, hresp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    let p = ui.painter_at(head);
    p.line_segment(
        [head.left_bottom(), head.right_bottom()],
        Stroke::new(1.0_f32, c(th.border)),
    );
    let mut heads = vec![
        (Col::Name, "Name"),
        (Col::Status, "Status"),
        (Col::Duration, "Duration"),
    ];
    if wide {
        heads.push((Col::Rate, "Rate"));
        heads.push((Col::Channels, "Ch"));
    }
    for (i, (col, label)) in heads.iter().enumerate() {
        let (x0, x1) = cols[i];
        let mark = match st.files_ui.sort {
            Some((c, false)) if c == *col => " ▲",
            Some((c, true)) if c == *col => " ▼",
            _ => "",
        };
        p.text(
            pos2(head.left() + x0 + 4.0, head.center().y),
            Align2::LEFT_CENTER,
            format!("{label}{mark}"),
            FontId::proportional(11.0),
            c(th.text_dim),
        );
        if hresp.clicked() {
            if let Some(pp) = hresp.interact_pointer_pos() {
                let x = pp.x - head.left();
                if x >= x0 && x < x1 {
                    st.files_ui.sort = next_sort(st.files_ui.sort, *col);
                }
            }
        }
    }

    let mut open_wave = None;
    let mut action: Option<Action> = None;
    egui::ScrollArea::vertical()
        .id_salt("af2-files-list")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for row in &rows {
                let picked = st.files_ui.selected.contains(&row.id);
                let payload = if picked {
                    st.files_ui.picked_in(&order)
                } else {
                    vec![row.id]
                };
                let on = st.current == Some(row.id) && st.mode == Mode::Waveform;
                let r = ui
                    .dnd_drag_source(egui::Id::new(("af2-file", row.id)), DragFiles(payload), |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
                        let p = ui.painter_at(rect);
                        if picked {
                            p.rect_filled(rect, Rounding::same(3.0), ca(th.selection, th.selection_alpha.max(60)));
                        }
                        let texts = [
                            format!(
                                "{}{}",
                                row.name,
                                if row.dirty { " *" } else { "" }
                            ),
                            status(row).to_string(),
                            format_time(row.dur),
                            format!("{:.1}k", row.rate as f64 / 1000.0),
                            row.ch.to_string(),
                        ];
                        for (i, (x0, x1)) in cols.iter().enumerate() {
                            let clip = Rect::from_min_max(
                                pos2(rect.left() + x0 + 4.0, rect.top()),
                                pos2(rect.left() + x1 - 2.0, rect.bottom()),
                            );
                            let color = if i == 0 && on {
                                c(th.accent)
                            } else if i == 0 {
                                c(th.text)
                            } else {
                                c(th.text_dim)
                            };
                            ui.painter_at(clip).text(
                                pos2(clip.left(), clip.center().y),
                                Align2::LEFT_CENTER,
                                &texts[i],
                                FontId::proportional(if i == 0 { 12.5 } else { 11.0 }),
                                color,
                            );
                        }
                    })
                    .response
                    .interact(Sense::click());
                let r = r.on_hover_text(format!(
                    "{}\n{} • {} Hz • {} ch{}\nDouble-click to edit; drag onto the Multitrack (Alt: one after another)",
                    st.doc_by_id(row.id)
                        .and_then(|d| d.path.as_ref())
                        .map_or("Not saved yet".into(), |p| p.display().to_string()),
                    format_time(row.dur),
                    row.rate,
                    row.ch,
                    if row.used { " • in a session" } else { "" }
                ));
                let m = ui.input(|i| i.modifiers);
                if r.clicked() {
                    st.files_ui.click(&order, row.id, m.command, m.shift);
                }
                if r.secondary_clicked() && !picked {
                    st.files_ui.click(&order, row.id, false, false);
                }
                if r.double_clicked() {
                    open_wave = Some(row.id);
                }
                r.context_menu(|ui| {
                    let n = st.files_ui.selected.len().max(1);
                    let files = if n == 1 { "File".to_string() } else { format!("{n} Files") };
                    if ui.button("Edit in Waveform Editor").clicked() {
                        open_wave = Some(row.id);
                        ui.close_menu();
                    }
                    if ui.button(format!("Insert {files} at Playhead")).on_hover_text("On the selected track, one after another").clicked() {
                        action = Some(Action::AtPlayhead);
                        ui.close_menu();
                    }
                    if ui.button(format!("Insert {files} as Tracks")).on_hover_text("Each on its own new track at the start, named without a leading speaker number").clicked() {
                        action = Some(Action::AsTracks);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(format!("Close {files}")).clicked() {
                        action = Some(Action::Close);
                        ui.close_menu();
                    }
                    if ui.button("Remove Unused Files").clicked() {
                        action = Some(Action::RemoveUnused);
                        ui.close_menu();
                    }
                    if ui.button("Select All").clicked() {
                        action = Some(Action::SelectAll);
                        ui.close_menu();
                    }
                    if let Some(p) = st.doc_by_id(row.id).and_then(|d| d.path.clone()) {
                        if ui.button("Show in File Manager").clicked() {
                            crate::sessions::reveal(&p);
                            ui.close_menu();
                        }
                    }
                });
            }
            for name in st.loading() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("{name} (reading)")).color(c(th.text_dim)));
                });
            }
            if st.docs.is_empty() && st.jobs.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Drop audio files or folders anywhere in the window.")
                        .color(c(th.text_dim)),
                );
            }
        });
    if let Some(id) = open_wave {
        st.current = Some(id);
        st.mode = Mode::Waveform;
    }
    let picked = st.files_ui.picked_in(&order);
    match action {
        Some(Action::AtPlayhead) => {
            let track = st
                .selected_track
                .or(st.session.tracks.first().map(|t| t.id));
            let at = st.session_cursor;
            st.import_docs(
                picked,
                Layout::Drop {
                    track,
                    at,
                    alt: true,
                },
            );
        }
        Some(Action::AsTracks) => st.import_docs(picked, Layout::AsTracks),
        Some(Action::Close) => {
            for d in picked {
                st.close_doc(d);
            }
        }
        Some(Action::RemoveUnused) => st.remove_unused_files(),
        Some(Action::SelectAll) => st.files_ui.selected = order.clone(),
        None => {}
    }
}

enum Action {
    AtPlayhead,
    AsTracks,
    Close,
    RemoveUnused,
    SelectAll,
}

fn status(r: &Row) -> &'static str {
    if r.dirty {
        "Edited"
    } else if r.used {
        "In use"
    } else {
        "Ready"
    }
}

/// Column spans (from, to) across the panel width.
fn columns(w: f32, wide: bool) -> Vec<(f32, f32)> {
    let fixed: &[f32] = if wide {
        &[52.0, 62.0, 50.0, 28.0]
    } else {
        &[52.0, 62.0]
    };
    let name = (w - fixed.iter().sum::<f32>()).max(60.0);
    let mut v = vec![(0.0, name)];
    let mut x = name;
    for f in fixed {
        v.push((x, x + f));
        x += f;
    }
    v
}

/// The open sessions, front first.
fn sessions(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let front = (!st.front_is_blank()).then(|| {
        (
            st.session.name.clone(),
            st.session_dirty,
            st.session.duration(),
            st.session.rate,
        )
    });
    let parked: Vec<(String, bool, f64, u32)> = st
        .sessions
        .parked
        .iter()
        .map(|p| {
            (
                p.session.name.clone(),
                p.dirty,
                p.session.duration(),
                p.session.rate,
            )
        })
        .collect();
    if front.is_none() && parked.is_empty() {
        return;
    }
    let mut act: Option<(Option<usize>, SessionAct)> = None;
    let rows = front
        .into_iter()
        .map(|f| (None, f))
        .chain(parked.into_iter().enumerate().map(|(i, p)| (Some(i), p)));
    for (which, (name, dirty, dur, rate)) in rows {
        let w = ui.available_width();
        let (rect, r) = ui.allocate_exact_size(vec2(w, ROW_H + 2.0), Sense::click());
        let p = ui.painter_at(rect);
        let in_front = which.is_none();
        if in_front {
            p.rect_filled(rect, Rounding::same(3.0), ca(th.accent, 28));
        }
        p.text(
            pos2(rect.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!(
                "{} {name}{}",
                if in_front { "▶" } else { "▷" },
                if dirty { " *" } else { "" }
            ),
            FontId::proportional(12.5),
            if in_front { c(th.accent) } else { c(th.text) },
        );
        p.text(
            pos2(rect.right() - 22.0, rect.center().y),
            Align2::RIGHT_CENTER,
            if in_front {
                format!("{} • {} Hz", format_time(dur), rate)
            } else {
                "Open".to_string()
            },
            FontId::proportional(10.5),
            c(th.text_dim),
        );
        let x =
            Rect::from_center_size(pos2(rect.right() - 10.0, rect.center().y), vec2(16.0, 16.0));
        let xr = ui.interact(
            x,
            ui.id().with(("af2-close-session", which)),
            Sense::click(),
        );
        p.text(
            x.center(),
            Align2::CENTER_CENTER,
            "×",
            FontId::proportional(14.0),
            if xr.hovered() {
                c(th.accent_alt)
            } else {
                c(th.text_dim)
            },
        );
        if xr.on_hover_text("Close this session").clicked() {
            act = Some((which, SessionAct::Close));
        }
        let r = r.on_hover_text(if in_front {
            "The session in the Multitrack. Double-click to show it."
        } else {
            "Set aside with its unsaved changes. Double-click to bring it to the front."
        });
        if r.double_clicked() {
            act = Some((which, SessionAct::Switch));
        }
        r.context_menu(|ui| {
            let label = if in_front {
                "Show in Multitrack"
            } else {
                "Switch to This Session"
            };
            if ui.button(label).clicked() {
                act = Some((which, SessionAct::Switch));
                ui.close_menu();
            }
            if ui.button("Save").clicked() {
                act = Some((which, SessionAct::Save));
                ui.close_menu();
            }
            if ui.button("Close Session").clicked() {
                act = Some((which, SessionAct::Close));
                ui.close_menu();
            }
            if ui.button("Close All Sessions").clicked() {
                act = Some((which, SessionAct::CloseAll));
                ui.close_menu();
            }
        });
    }
    ui.add_space(2.0);
    match act {
        Some((None, SessionAct::Switch)) => st.mode = Mode::Multitrack,
        Some((Some(i), SessionAct::Switch)) => st.switch_session(i),
        Some((which, SessionAct::Save)) => {
            if let Some(i) = which {
                st.switch_session(i);
            }
            st.save_front(false);
        }
        Some((None, SessionAct::Close)) => {
            st.close_front();
        }
        Some((Some(i), SessionAct::Close)) => {
            st.close_parked(i);
        }
        Some((_, SessionAct::CloseAll)) => {
            st.close_all_sessions();
        }
        None => {}
    }
}

enum SessionAct {
    Switch,
    Save,
    Close,
    CloseAll,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_sort_up_down_then_opening_order() {
        let s = next_sort(None, Col::Name);
        assert_eq!(s, Some((Col::Name, false)));
        let s = next_sort(s, Col::Name);
        assert_eq!(s, Some((Col::Name, true)));
        assert_eq!(next_sort(s, Col::Name), None);
        assert_eq!(next_sort(s, Col::Duration), Some((Col::Duration, false)));
    }

    #[test]
    fn clicks_pick_files() {
        let order = [5, 3, 9, 1];
        let mut f = FilesUi::default();
        f.click(&order, 3, false, false);
        assert_eq!(f.selected, [3]);
        // Shift: the run from the anchor, either way.
        f.click(&order, 1, false, true);
        assert_eq!(f.selected, [3, 9, 1]);
        f.click(&order, 5, false, true);
        assert_eq!(f.selected, [5, 3]);
        // Ctrl adds and removes, and moves the anchor.
        f.click(&order, 1, true, false);
        assert_eq!(f.picked_in(&order), [5, 3, 1]);
        f.click(&order, 3, true, false);
        assert_eq!(f.picked_in(&order), [5, 1]);
        f.forget(1);
        assert_eq!(f.selected, [5]);
        assert_eq!(range_select(&order, 42, 9), [9]);
    }
}
