// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Updates: a look at the project's latest GitHub release a few seconds
//! after start (Preferences > General can turn this off) or from Help >
//! Check for Updates. A newer version shows a small window with its notes;
//! on Windows "Update and Restart" downloads the release's program with the
//! system's own `curl`, checks it against the SHA-256 GitHub lists for it,
//! puts it in place of the running program (which Windows allows by
//! renaming the running file aside) and starts it. Elsewhere it opens the
//! release page. Nothing is sent but the request itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::state::State;

/// The project's latest release, as GitHub's API gives it.
const LATEST: &str = "https://api.github.com/repos/AudiFormer/AudiForm2/releases/latest";
/// Where releases are listed (for "Release Notes" and other systems).
pub const RELEASES: &str = "https://github.com/AudiFormer/AudiForm2/releases";
/// The release file this build installs itself from.
const ASSET_SUFFIX: &str = "-windows-x64.exe";

/// A newer release.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub notes: String,
    /// The Windows program: (download URL, size in bytes, SHA-256 hex).
    pub asset: Option<(String, u64, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    /// Put in place; restarting.
    Installed(String),
    Failed(String),
}

/// The updater's state in the app.
#[derive(Default)]
pub struct Updater {
    phase: Arc<Mutex<Phase>>,
    /// Asked from the Help menu: say "up to date" and errors too.
    manual: bool,
    /// The window is shown (closing it hides an available update until
    /// the next start).
    pub open: bool,
    started: bool,
}

impl Updater {
    pub fn phase(&self) -> Phase {
        self.phase.lock().map(|p| p.clone()).unwrap_or_default()
    }

    fn set(&self, p: Phase) {
        if let Ok(mut g) = self.phase.lock() {
            *g = p;
        }
    }

    /// Looks for a newer release in the background.
    pub fn check(&mut self, manual: bool) {
        if matches!(self.phase(), Phase::Checking | Phase::Downloading(_)) {
            return;
        }
        self.manual = manual;
        self.open = manual;
        self.set(Phase::Checking);
        let phase = self.phase.clone();
        std::thread::spawn(move || {
            let p = match fetch_latest() {
                Ok(Some(r)) => Phase::Available(r),
                Ok(None) => Phase::UpToDate,
                Err(e) => Phase::Failed(e),
            };
            if let Ok(mut g) = phase.lock() {
                *g = p;
            }
        });
    }

    /// Downloads `r`'s program, puts it in place and restarts.
    pub fn install(&mut self, r: Release) {
        let Some((url, size, sha)) = r.asset.clone() else {
            return;
        };
        self.set(Phase::Downloading(r.clone()));
        let phase = self.phase.clone();
        std::thread::spawn(move || {
            let p = match download_and_swap(&url, size, sha.as_deref()) {
                Ok(()) => Phase::Installed(r.version.clone()),
                Err(e) => Phase::Failed(e),
            };
            if let Ok(mut g) = phase.lock() {
                *g = p;
            }
        });
    }
}

/// "2.1.0" or "v2.1.0" as numbers (anything after a '-' is ignored).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().unwrap_or(Ok(0)).ok()?;
    let c = it.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

/// Whether release `tag` is newer than this build.
pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(t), Some(c)) => t > c,
        _ => false,
    }
}

/// The release from GitHub's JSON, if newer than `current`.
pub fn release_from_json(json: &str, current: &str) -> Result<Option<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("The release has no version tag.")?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    if !is_newer(tag, current) {
        return Ok(None);
    }
    let asset = v["assets"].as_array().and_then(|a| {
        a.iter().find_map(|x| {
            let name = x["name"].as_str()?;
            if !name.ends_with(ASSET_SUFFIX) {
                return None;
            }
            let url = x["browser_download_url"].as_str()?.to_string();
            let size = x["size"].as_u64().unwrap_or(0);
            let sha = x["digest"]
                .as_str()
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(str::to_lowercase);
            Some((url, size, sha))
        })
    });
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: v["html_url"].as_str().unwrap_or(RELEASES).to_string(),
        notes: v["body"].as_str().unwrap_or("").to_string(),
        asset,
    }))
}

fn curl() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("curl.exe"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("curl"))
    } else {
        PathBuf::from("curl")
    }
}

fn hidden(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up.
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd;
}

fn fetch_latest() -> Result<Option<Release>, String> {
    let mut cmd = Command::new(curl());
    cmd.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--max-time",
        "20",
        "-H",
        "Accept: application/vnd.github+json",
        "-A",
        "AudiForm2-updater",
        LATEST,
    ])
    .stdin(Stdio::null());
    hidden(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("Couldn't check for updates ({e})."))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if why.is_empty() {
            "Couldn't reach GitHub to check for updates.".into()
        } else {
            format!("Couldn't reach GitHub to check for updates: {why}")
        });
    }
    release_from_json(&String::from_utf8_lossy(&out.stdout), env!("CARGO_PKG_VERSION"))
}

fn sha256_file(p: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The running program renamed aside while its replacement is put in.
fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("old")
}

fn download_and_swap(url: &str, size: u64, sha: Option<&str>) -> Result<(), String> {
    if !url.starts_with("https://github.com/") && !url.starts_with("https://objects.githubusercontent.com/") {
        return Err("The update isn't from GitHub, so it wasn't downloaded.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let part = exe.with_extension("download");
    let _ = std::fs::remove_file(&part);
    let mut cmd = Command::new(curl());
    cmd.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "-A",
        "AudiForm2-updater",
        "--output",
    ])
    .arg(&part)
    .arg(url)
    .stdin(Stdio::null());
    hidden(&mut cmd);
    let out = cmd.output().map_err(|e| format!("The download didn't start ({e})."))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "The download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if got == 0 || (size > 0 && got != size) {
        let _ = std::fs::remove_file(&part);
        return Err("The download came out the wrong size, so it wasn't used.".into());
    }
    if let Some(want) = sha {
        let have = sha256_file(&part)?;
        if have != want {
            let _ = std::fs::remove_file(&part);
            return Err("The download didn't match its checksum, so it wasn't used.".into());
        }
    }
    // Windows lets a running program be renamed but not overwritten.
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Couldn't make room for the update ({e})."))?;
    if let Err(e) = std::fs::rename(&part, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Couldn't put the update in place ({e})."));
    }
    Ok(())
}

/// At start: the program set aside by the last update is removed.
pub fn tidy_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
        let _ = std::fs::remove_file(exe.with_extension("download"));
    }
}

/// Every frame: the check a few seconds after start, and the window.
pub fn ui(ctx: &egui::Context, st: &mut State) {
    if !st.updater.started {
        st.updater.started = true;
        tidy_after_update();
    }
    let since = ctx.input(|i| i.time);
    if st.prefs.check_updates && !st.update_checked && since > 4.0 {
        st.update_checked = true;
        st.updater.check(false);
    }
    let phase = st.updater.phase();
    match &phase {
        Phase::Checking | Phase::Downloading(_) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(250))
        }
        // An automatic check opens the window only for a new version not skipped.
        Phase::Available(r) if !st.updater.manual => {
            if st.prefs.skip_update_version != r.version && !st.update_shown {
                st.update_shown = true;
                st.updater.open = true;
            }
        }
        Phase::Installed(v) => {
            let v = v.clone();
            st.updater.set(Phase::Idle);
            restart(ctx, st, &v);
            return;
        }
        _ => {}
    }
    if !st.updater.open {
        return;
    }
    // A failed or up-to-date automatic check stays quiet.
    if !st.updater.manual && matches!(phase, Phase::UpToDate | Phase::Failed(_)) {
        st.updater.open = false;
        return;
    }
    let mut open = true;
    let mut act: Option<&str> = None;
    crate::ui::popup(ctx, egui::Id::new("af2-updates"), "Updates")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            let now = env!("CARGO_PKG_VERSION");
            match &phase {
                Phase::Idle | Phase::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking for a newer version...");
                    });
                }
                Phase::UpToDate => {
                    ui.label(format!("AudiForm {now} is the latest version."));
                }
                Phase::Failed(e) => {
                    ui.label(e);
                    if ui.button("Open the Releases Page").clicked() {
                        act = Some("page");
                    }
                }
                Phase::Available(r) => {
                    ui.label(
                        egui::RichText::new(format!("AudiForm {} is available", r.version))
                            .strong()
                            .size(15.0),
                    );
                    ui.label(format!("You have {now}."));
                    if std::env::var_os("FLATPAK_ID").is_some() {
                        ui.label(
                            egui::RichText::new(
                                "Installed as a Flatpak: download the new .flatpak from the release page and open it to update.",
                            )
                            .small(),
                        );
                    } else if std::env::var_os("APPIMAGE").is_some() {
                        ui.label(
                            egui::RichText::new(
                                "Running as an AppImage: download the new .AppImage from the release page and use it in place of this one.",
                            )
                            .small(),
                        );
                    }
                    if !r.notes.trim().is_empty() {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.label(egui::RichText::new(notes_text(&r.notes)).size(11.5));
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if cfg!(windows) && r.asset.is_some() {
                            if ui
                                .button(egui::RichText::new("Update and Restart").strong())
                                .on_hover_text("Downloads the new version, checks it, replaces this one and starts it. Unsaved work is asked about first.")
                                .clicked()
                            {
                                act = Some("install");
                            }
                        } else if ui.button("Download").clicked() {
                            act = Some("page");
                        }
                        if ui.button("Release Notes").clicked() {
                            act = Some("page");
                        }
                        if ui.button("Skip This Version").clicked() {
                            act = Some("skip");
                        }
                        if ui.button("Later").clicked() {
                            act = Some("later");
                        }
                    });
                }
                Phase::Downloading(r) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Downloading AudiForm {}...", r.version));
                    });
                }
                Phase::Installed(_) => {}
            }
        });
    let page = match &phase {
        Phase::Available(r) | Phase::Downloading(r) => r.page.clone(),
        _ => RELEASES.to_string(),
    };
    match act {
        Some("install") => {
            if let Phase::Available(r) = &phase {
                st.updater.install(r.clone());
            }
        }
        Some("page") => ctx.open_url(egui::OpenUrl::new_tab(page)),
        Some("skip") => {
            if let Phase::Available(r) = &phase {
                st.prefs.skip_update_version = r.version.clone();
                st.prefs.save();
            }
            open = false;
        }
        Some("later") => open = false,
        _ => {}
    }
    if !open && !matches!(phase, Phase::Downloading(_)) {
        st.updater.open = false;
    }
}

/// Release notes as plain text: Markdown's marks taken out.
fn notes_text(md: &str) -> String {
    md.lines()
        .map(|l| {
            let l = l.trim_start_matches('#').trim_start();
            l.replace("**", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The new program is in place: unsaved work is asked about, then it is
/// started and this one closes.
fn restart(ctx: &egui::Context, st: &mut State, version: &str) {
    if !st.may_quit() {
        st.toast(format!(
            "AudiForm {version} is installed and starts the next time you open AudiForm 2."
        ));
        return;
    }
    let started = std::env::current_exe()
        .ok()
        .and_then(|exe| Command::new(exe).spawn().ok())
        .is_some();
    if !started {
        st.toast(format!(
            "AudiForm {version} is installed. Start AudiForm 2 again to use it."
        ));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("v2.1.0", "2.0.3"));
        assert!(is_newer("2.0.10", "2.0.9"));
        assert!(!is_newer("v2.0.3", "2.0.3"));
        assert!(!is_newer("v1.9.9", "2.0.0"));
        assert!(!is_newer("nightly", "2.0.0"));
        assert_eq!(parse_version("v2.1"), Some((2, 1, 0)));
        assert_eq!(parse_version("2.1.0-beta.1"), Some((2, 1, 0)));
    }

    #[test]
    fn a_newer_release_is_read_with_its_windows_program() {
        let json = r#"{
            "tag_name": "v2.1.0", "html_url": "https://github.com/AudiFormer/AudiForm2/releases/tag/v2.1.0",
            "body": "notes", "draft": false, "prerelease": false,
            "assets": [
                {"name": "AudiForm2-2.1.0-linux-x64.tar.gz", "browser_download_url": "https://github.com/x/l", "size": 5},
                {"name": "AudiForm2-2.1.0-windows-x64.exe", "browser_download_url": "https://github.com/x/w", "size": 42,
                 "digest": "sha256:ABCDEF"}
            ]}"#;
        let r = release_from_json(json, "2.0.3").unwrap().unwrap();
        assert_eq!(r.version, "2.1.0");
        assert_eq!(
            r.asset,
            Some(("https://github.com/x/w".into(), 42, Some("abcdef".into())))
        );
        // The same release from itself, or a pre-release: nothing to do.
        assert!(release_from_json(json, "2.1.0").unwrap().is_none());
        let pre = json.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(release_from_json(&pre, "2.0.3").unwrap().is_none());
        assert!(release_from_json("not json", "2.0.3").is_err());
    }

    #[test]
    fn notes_lose_their_markdown_marks() {
        assert_eq!(notes_text("# Title\n- **Bold** `code`"), "Title\n- Bold code");
    }
}
