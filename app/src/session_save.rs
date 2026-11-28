// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Save As for a session: file name, location (recent folders and
//! Browse), format, the session's sample rate, and whether markers go in;
//! OK writes it.

use std::path::{Path, PathBuf};

use egui::RichText;

use crate::state::State;
use crate::theme::c;

/// What the session is saved as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The session's own file (it is then the session's file).
    Audfrm,
    /// A copy for Audition; the session keeps its own file.
    Sesx,
    /// AudiForm 2's model, as plain JSON.
    Dump,
}

impl Format {
    const ALL: [Format; 3] = [Format::Audfrm, Format::Sesx, Format::Dump];

    fn label(self) -> &'static str {
        match self {
            Format::Audfrm => "AudiForm Session (*.audfrm)",
            Format::Sesx => "Audition Session (*.sesx), a copy",
            Format::Dump => "AudiForm 2 model dump (*.af2)",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Format::Audfrm => "audfrm",
            Format::Sesx => "sesx",
            Format::Dump => "af2",
        }
    }
}

/// The open dialog.
pub struct SessionSave {
    pub name: String,
    pub folder: PathBuf,
    pub format: Format,
    pub markers: bool,
    /// A file there already: asked before writing over it.
    confirm: bool,
}

/// How many folders the Location list keeps.
const RECENT: usize = 8;

/// Opens Save As for the session in front.
pub fn open(st: &mut State) {
    let folder = st
        .session_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| st.prefs.session_folder.clone().filter(|f| f.is_dir()))
        .or_else(dirs_home)
        .unwrap_or_else(|| PathBuf::from("."));
    st.session_save = Some(SessionSave {
        name: af2_io::export::safe_file_name(&st.session.name),
        folder,
        format: Format::Audfrm,
        markers: true,
        confirm: false,
    });
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// A long path cut in the middle.
fn short(p: &Path, max: usize) -> String {
    let s = p.display().to_string();
    let n = s.chars().count();
    if n <= max {
        return s;
    }
    let keep = max.saturating_sub(1) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(n - keep).collect();
    format!("{head}\u{2026}{tail}")
}

/// Draws the dialog while it is open.
pub fn show(ctx: &egui::Context, st: &mut State) {
    let Some(mut d) = st.session_save.take() else { return };
    let th = st.theme.clone();
    let mut open = true;
    let mut ok = false;
    let mut cancel = false;
    let mut replace = false;
    let path = d.folder.join(format!("{}.{}", d.name.trim(), d.format.ext()));
    crate::ui::popup(ctx, egui::Id::new("af2-session-save"), "Save As")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("session-save-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("File Name:");
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(240.0));
                    if r.changed() {
                        d.confirm = false;
                    }
                    ui.label(RichText::new(format!(".{}", d.format.ext())).color(c(th.text_dim)));
                });
                ui.end_row();
                ui.label("Location:");
                ui.horizontal(|ui| {
                    let mut list = vec![d.folder.clone()];
                    for f in &st.prefs.session_recent {
                        if !list.contains(f) {
                            list.push(f.clone());
                        }
                    }
                    egui::ComboBox::from_id_salt("session-save-folder")
                        .selected_text(short(&d.folder, 40))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for f in list {
                                let on = f == d.folder;
                                if ui
                                    .selectable_label(on, short(&f, 60))
                                    .on_hover_text(f.display().to_string())
                                    .clicked()
                                {
                                    d.folder = f;
                                    d.confirm = false;
                                }
                            }
                        });
                    if ui.button("Browse...").clicked() {
                        if let Some(f) = crate::file_dialog::FileDialog::new().set_directory(&d.folder).pick_folder() {
                            d.folder = f;
                            d.confirm = false;
                        }
                    }
                });
                ui.end_row();
                ui.label("Format:");
                egui::ComboBox::from_id_salt("session-save-format")
                    .selected_text(d.format.label())
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for f in Format::ALL {
                            if ui.selectable_value(&mut d.format, f, f.label()).changed() {
                                d.confirm = false;
                            }
                        }
                    });
                ui.end_row();
                ui.label(RichText::new("Sample Type:").color(c(th.text_dim)));
                let tracks = st.session.tracks.len();
                ui.label(
                    RichText::new(format!(
                        "{} Hz, {} track{}",
                        st.session.rate,
                        tracks,
                        if tracks == 1 { "" } else { "s" }
                    ))
                    .color(c(th.text_dim)),
                );
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.checkbox(&mut d.markers, "Include markers and other metadata");
            if d.format == Format::Sesx {
                ui.label(
                    RichText::new("Saves a copy for Audition; this session keeps its own file.")
                        .small()
                        .color(c(th.text_dim)),
                );
            }
            ui.add_space(8.0);
            if d.confirm {
                ui.label(RichText::new(format!("{} already exists. Replace it?", short(&path, 60))).strong());
                ui.horizontal(|ui| {
                    replace = ui.button("Replace").clicked();
                    if ui.button("Cancel").clicked() {
                        d.confirm = false;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    // OK and Cancel at the right, under the fields.
                    ui.add_space(250.0);
                    let can = !d.name.trim().is_empty();
                    ok = ui.add_enabled(can, crate::quit_flow::main_button(&th, "OK")).clicked();
                    cancel = ui.add_sized([86.0, 26.0], egui::Button::new("Cancel")).clicked();
                });
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !d.name.trim().is_empty() {
                    ok = true;
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if !open || cancel {
        return;
    }
    if ok {
        if path.exists() && !st.session_path.as_deref().is_some_and(|p| crate::sessions::same_path(p, &path)) {
            d.confirm = true;
            st.session_save = Some(d);
            return;
        }
        replace = true;
    }
    if replace {
        save(st, &d, path);
        return;
    }
    st.session_save = Some(d);
}

/// Writes the session as asked, and remembers its folder.
fn save(st: &mut State, d: &SessionSave, path: PathBuf) {
    let folder = d.folder.clone();
    // Without markers: left out of the file, kept in the session.
    let kept = if d.markers { None } else { Some(std::mem::take(&mut st.session.markers)) };
    match d.format {
        Format::Sesx => {
            crate::plugins::capture_all(st);
            match af2_interop::sesx_out::export(&st.session, &path) {
                Ok(report) => {
                    st.toast(format!("Saved a copy as {}", path.file_name().unwrap_or_default().to_string_lossy()));
                    if let Some(n) = report.notice() {
                        st.toast(n);
                    }
                }
                Err(e) => st.error(e),
            }
        }
        Format::Audfrm | Format::Dump => st.save_session_to(path),
    }
    if let Some(m) = kept {
        let had = !m.is_empty();
        st.session.markers = m;
        if had && d.format != Format::Sesx {
            // The file has none; the session still has them.
            st.session_dirty = true;
        }
    }
    st.prefs.session_folder = Some(folder.clone());
    st.prefs.session_recent.retain(|f| f != &folder);
    st.prefs.session_recent.insert(0, folder);
    st.prefs.session_recent.truncate(RECENT);
    st.prefs.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_as_opens_with_the_session_name_and_its_own_format() {
        let mut st = State::new();
        st.session.name = "Demo: take 2".into();
        open(&mut st);
        let d = st.session_save.as_ref().expect("the dialog opens");
        assert!(!d.name.contains(':'), "a name a file can have: {}", d.name);
        assert_eq!(d.format, Format::Audfrm);
        assert!(d.markers);
        // Saving as before (not Save As) a session with no file opens it too.
        st.session_save = None;
        assert!(!st.save_front(false));
        assert!(st.session_save.is_some());
    }
}
