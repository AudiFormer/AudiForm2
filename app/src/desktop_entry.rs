// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The applications menu, when running from an AppImage. An AppImage is a
//! single file nothing installs, so the desktop doesn't know about it; this
//! writes a desktop entry and icon into `~/.local/share` that start the
//! AppImage where it is. The first start asks; Help > Add to Applications
//! Menu (or Remove) does it any time. An entry made here follows the
//! AppImage when it is moved or a newer one is started, and `TryExec` hides
//! it from the menu once the file is gone.

use std::path::{Path, PathBuf};

use crate::state::{data_dir, State};

const ID: &str = "io.github.audiformer.AudiForm2";
/// The packaging's own desktop entry, with `Exec` replaced here.
const TEMPLATE: &str = include_str!("../../packaging/flatpak/io.github.audiformer.AudiForm2.desktop");
/// Marks an entry as this one's, so nothing else's is changed or removed.
const MARK: &str = "X-AudiForm2-AppImage=true";
/// The first start's question has been answered.
const ASKED: &str = "appimage-menu-asked";

#[derive(Default)]
pub struct Ui {
    started: bool,
    /// The first start's question is shown.
    pub asking: bool,
    /// What the last Add or Remove did, shown in the question's place.
    result: Option<String>,
}

/// The AppImage this program runs from (its runtime sets `$APPIMAGE`).
pub fn appimage() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let p = PathBuf::from(std::env::var_os("APPIMAGE")?);
    p.is_file().then_some(p)
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn entry_path() -> Option<PathBuf> {
    Some(data_home()?.join("applications").join(format!("{ID}.desktop")))
}

fn icon_path() -> Option<PathBuf> {
    Some(data_home()?.join("icons/hicolor/256x256/apps").join(format!("{ID}.png")))
}

/// This program's entry, if one is there.
fn ours() -> Option<String> {
    let text = std::fs::read_to_string(entry_path()?).ok()?;
    text.lines().any(|l| l.trim() == MARK).then_some(text)
}

pub fn installed() -> bool {
    ours().is_some()
}

/// A desktop entry string value: backslashes and line breaks escaped.
fn value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

/// The AppImage as the program of an `Exec` line: quoted, with the
/// characters the spec reserves inside quotes escaped, and `%` doubled.
fn exec_arg(p: &Path) -> String {
    let mut q = String::from("\"");
    for ch in p.to_string_lossy().chars() {
        match ch {
            '"' | '`' | '$' | '\\' => {
                q.push('\\');
                q.push(ch);
            }
            '%' => q.push_str("%%"),
            _ => q.push(ch),
        }
    }
    q.push('"');
    value(&q)
}

fn entry_text(app: &Path, icon: &Path) -> String {
    let mut out = String::new();
    for line in TEMPLATE.lines() {
        if line.starts_with("#!") {
            continue;
        }
        if line.starts_with("Exec=") {
            out.push_str(&format!("TryExec={}\n", value(&app.to_string_lossy())));
            out.push_str(&format!("Exec={} %F\n", exec_arg(app)));
        } else if line.starts_with("Icon=") {
            out.push_str(&format!("Icon={}\n", value(&icon.to_string_lossy())));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(MARK);
    out.push('\n');
    out
}

/// Writes the entry and icon for `app`.
pub fn add(app: &Path) -> Result<(), String> {
    let entry = entry_path().ok_or("No home folder to add it to.")?;
    let icon = icon_path().ok_or("No home folder to add it to.")?;
    if entry.exists() && !installed() {
        return Err(format!(
            "There's already an AudiForm 2 entry at {} (from another install); it was left as it is.",
            entry.display()
        ));
    }
    let write = |p: &Path, b: &[u8]| -> Result<(), String> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("Couldn't make {}: {e}", d.display()))?;
        }
        std::fs::write(p, b).map_err(|e| format!("Couldn't write {}: {e}", p.display()))
    };
    write(&icon, crate::app_icon::DEFAULT_256)?;
    write(&entry, entry_text(app, &icon).as_bytes())?;
    refresh();
    Ok(())
}

/// Removes this program's entry and icon (never another install's).
pub fn remove() -> Result<(), String> {
    if !installed() {
        return Ok(());
    }
    let entry = entry_path().ok_or("No home folder.")?;
    std::fs::remove_file(&entry).map_err(|e| format!("Couldn't remove {}: {e}", entry.display()))?;
    if let Some(icon) = icon_path() {
        let _ = std::fs::remove_file(icon);
    }
    refresh();
    Ok(())
}

/// Tells menus that read the cache (most watch the folder anyway).
fn refresh() {
    if let Some(dir) = entry_path().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let _ = std::process::Command::new("update-desktop-database")
            .arg(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Once, at start: point an existing entry at this AppImage, or ask.
fn start(st: &mut State) {
    let Some(app) = appimage() else { return };
    if let Some(text) = ours() {
        let want = format!("Exec={} %F", exec_arg(&app));
        if !text.lines().any(|l| l == want) {
            let _ = add(&app);
        }
        return;
    }
    let entry_taken = entry_path().is_some_and(|p| p.exists());
    st.desktop_entry.asking = !entry_taken && !data_dir().join(ASKED).exists();
}

fn remember_asked() {
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(data_dir().join(ASKED), b"");
}

/// The Help menu's item; shown only when running from an AppImage.
pub fn menu_item(ui: &mut egui::Ui, st: &mut State) {
    let Some(app) = appimage() else { return };
    if installed() {
        if ui
            .button("Remove from Applications Menu")
            .on_hover_text("Takes AudiForm 2 out of your desktop's applications menu. The AppImage itself stays.")
            .clicked()
        {
            st.desktop_entry.result = Some(match remove() {
                Ok(()) => "AudiForm 2 was removed from your applications menu.".into(),
                Err(e) => e,
            });
            ui.close_menu();
        }
    } else if ui
        .button("Add to Applications Menu")
        .on_hover_text("Lists AudiForm 2 in your desktop's applications menu, starting this AppImage from where it is.")
        .clicked()
    {
        st.desktop_entry.result = Some(match add(&app) {
            Ok(()) => "AudiForm 2 is in your applications menu.".into(),
            Err(e) => e,
        });
        ui.close_menu();
    }
}

/// The first start's question, and what Add or Remove did.
pub fn ui(ctx: &egui::Context, st: &mut State) {
    if !st.desktop_entry.started {
        st.desktop_entry.started = true;
        start(st);
    }
    if let Some(msg) = st.desktop_entry.result.clone() {
        let mut open = true;
        let mut ok = false;
        crate::ui::popup(ctx, egui::Id::new("af2-desktop-entry-result"), "Applications Menu")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(msg);
                ok = ui.button("OK").clicked();
            });
        if !open || ok {
            st.desktop_entry.result = None;
        }
        return;
    }
    if !st.desktop_entry.asking {
        return;
    }
    let Some(app) = appimage() else {
        st.desktop_entry.asking = false;
        return;
    };
    let mut open = true;
    let mut act = None;
    crate::ui::popup(ctx, egui::Id::new("af2-desktop-entry-ask"), "Add to Applications Menu?")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label("Add AudiForm 2 to your applications menu, so you can find it there next time?");
            ui.label(
                egui::RichText::new(format!(
                    "It starts this AppImage from where it is now ({}). If you move it, start it once from the new place and the menu follows.",
                    app.display()
                ))
                .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(egui::RichText::new("Add").strong()).clicked() {
                    act = Some("add");
                }
                if ui.button("Not Now").clicked() {
                    act = Some("later");
                }
                if ui
                    .button("Don't Ask Again")
                    .on_hover_text("Help > Add to Applications Menu still does it.")
                    .clicked()
                {
                    act = Some("never");
                }
            });
        });
    match act {
        Some("add") => {
            remember_asked();
            st.desktop_entry.asking = false;
            st.desktop_entry.result = Some(match add(&app) {
                Ok(()) => "AudiForm 2 is in your applications menu.".into(),
                Err(e) => e,
            });
        }
        Some("never") => {
            remember_asked();
            st.desktop_entry.asking = false;
        }
        Some("later") => st.desktop_entry.asking = false,
        _ if !open => st.desktop_entry.asking = false,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_runs_the_appimage() {
        let t = entry_text(
            Path::new("/home/me/Apps/AudiForm2-2.1.0-linux-x64.AppImage"),
            Path::new("/home/me/.local/share/icons/x.png"),
        );
        assert!(t.contains("\nExec=\"/home/me/Apps/AudiForm2-2.1.0-linux-x64.AppImage\" %F\n"));
        assert!(t.contains("\nTryExec=/home/me/Apps/AudiForm2-2.1.0-linux-x64.AppImage\n"));
        assert!(t.contains("\nIcon=/home/me/.local/share/icons/x.png\n"));
        assert!(t.starts_with("[Desktop Entry]\n"));
        assert!(t.ends_with(&format!("{MARK}\n")));
    }

    #[test]
    fn awkward_paths_are_escaped() {
        assert_eq!(exec_arg(Path::new("/a b/100%")), "\"/a b/100%%\"");
        // `$` becomes `\$` inside the quotes, then `\\$` as a string value.
        assert_eq!(exec_arg(Path::new("/x$y")), "\"/x\\\\$y\"");
    }
}
