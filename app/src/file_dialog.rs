// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Open, save and folder dialogs. The same calls as `rfd::FileDialog`; on
//! a KDE Plasma desktop with `kdialog` installed they are Plasma's own
//! dialogs (the Dolphin file view, places and previews), and everywhere
//! else, or when `kdialog` cannot be run, they are `rfd`'s.

use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct FileDialog {
    filters: Vec<(String, Vec<String>)>,
    directory: Option<PathBuf>,
    file_name: Option<String>,
    title: Option<String>,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl ToString]) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(|e| e.to_string()).collect(),
        ));
        self
    }

    pub fn set_directory<P: AsRef<Path>>(mut self, path: P) -> Self {
        let p = path.as_ref();
        if !p.as_os_str().is_empty() {
            self.directory = Some(p.to_path_buf());
        }
        self
    }

    pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    pub fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        match self.kde(Kind::Open) {
            Some(r) => r.into_iter().next(),
            None => self.rfd().pick_file(),
        }
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        match self.kde(Kind::OpenMany) {
            Some(r) => (!r.is_empty()).then_some(r),
            None => self.rfd().pick_files(),
        }
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        match self.kde(Kind::Folder) {
            Some(r) => r.into_iter().next(),
            None => self.rfd().pick_folder(),
        }
    }

    pub fn save_file(self) -> Option<PathBuf> {
        match self.kde(Kind::Save) {
            Some(r) => r.into_iter().next().map(|p| self.with_extension(p)),
            None => self.rfd().save_file(),
        }
    }

    fn rfd(&self) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        for (name, exts) in &self.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(p) = &self.directory {
            d = d.set_directory(p);
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n);
        }
        if let Some(t) = &self.title {
            d = d.set_title(t);
        }
        d
    }

    /// A saved name with no extension gets the first filter's, as the
    /// other dialogs do.
    fn with_extension(&self, p: PathBuf) -> PathBuf {
        if p.extension().is_some() {
            return p;
        }
        match self.filters.first().and_then(|(_, e)| e.first()) {
            Some(ext) if ext != "*" => p.with_extension(ext),
            _ => p,
        }
    }

    /// Plasma's dialog through `kdialog`: Some(paths) when it ran (empty
    /// when cancelled), None when this is not Plasma or it would not run.
    fn kde(&self, kind: Kind) -> Option<Vec<PathBuf>> {
        if !use_kdialog() {
            return None;
        }
        let args = self.kdialog_args(kind);
        let out = std::process::Command::new("kdialog")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        // 1 is Cancel; anything else odd (a crash, a missing display)
        // falls back to the other dialog.
        match out.status.code() {
            Some(0) => Some(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .collect(),
            ),
            Some(1) => Some(Vec::new()),
            _ => None,
        }
    }

    fn kdialog_args(&self, kind: Kind) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(t) = &self.title {
            a.push("--title".into());
            a.push(t.clone());
        }
        // The starting place: the folder, with the suggested name in it.
        let dir = self
            .directory
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let start = match (&kind, &self.file_name) {
            (Kind::Save | Kind::Open, Some(n)) => dir.join(n),
            _ => dir,
        };
        match kind {
            Kind::Open => a.push("--getopenfilename".into()),
            Kind::OpenMany => {
                a.push("--getopenfilename".into());
                a.push("--multiple".into());
                a.push("--separate-output".into());
            }
            Kind::Save => a.push("--getsavefilename".into()),
            Kind::Folder => a.push("--getexistingdirectory".into()),
        }
        a.push(start.to_string_lossy().into_owned());
        if !matches!(kind, Kind::Folder) {
            if let Some(f) = self.kdialog_filter() {
                a.push(f);
            }
        }
        a
    }

    /// Filters as KDE writes them: `*.wav *.flac|Audio files`, one per
    /// line.
    fn kdialog_filter(&self) -> Option<String> {
        if self.filters.is_empty() {
            return None;
        }
        let lines: Vec<String> = self
            .filters
            .iter()
            .map(|(name, exts)| {
                let pats: Vec<String> = exts
                    .iter()
                    .map(|e| if e == "*" { "*".to_string() } else { format!("*.{e}") })
                    .collect();
                format!("{}|{}", pats.join(" "), name.replace('|', "/"))
            })
            .collect();
        Some(lines.join("\n"))
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Open,
    OpenMany,
    Save,
    Folder,
}

/// On a Plasma desktop with `kdialog` on the PATH. `AF2_FILE_DIALOG=rfd`
/// turns it off; `=kdialog` asks for it on any desktop.
fn use_kdialog() -> bool {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return false;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let want = std::env::var("AF2_FILE_DIALOG").unwrap_or_default();
        if want.eq_ignore_ascii_case("rfd") {
            return false;
        }
        let plasma = want.eq_ignore_ascii_case("kdialog") || is_plasma();
        plasma && on_path("kdialog")
    })
}

/// The desktop is KDE Plasma.
pub fn is_plasma() -> bool {
    let has = |var: &str| {
        std::env::var(var)
            .unwrap_or_default()
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("kde"))
    };
    has("XDG_CURRENT_DESKTOP")
        || has("XDG_SESSION_DESKTOP")
        || std::env::var_os("KDE_FULL_SESSION").is_some()
}

/// A program by that name is on the PATH.
pub fn on_path(prog: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|d| d.join(prog).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdialog_gets_kde_filters_and_the_suggested_name() {
        let d = FileDialog::new()
            .add_filter("Audio", &["wav", "flac"])
            .add_filter("All files", &["*"])
            .set_directory("/home/me/Music")
            .set_file_name("Mix.wav")
            .set_title("Export");
        let a = d.kdialog_args(Kind::Save);
        assert_eq!(a[0], "--title");
        assert_eq!(a[1], "Export");
        assert_eq!(a[2], "--getsavefilename");
        assert!(a[3].ends_with("Mix.wav"), "{}", a[3]);
        assert_eq!(a[4], "*.wav *.flac|Audio\n*|All files");
        let f = FileDialog::new().set_directory("/tmp").kdialog_args(Kind::Folder);
        assert_eq!(f, vec!["--getexistingdirectory".to_string(), "/tmp".into()]);
    }

    #[test]
    fn a_saved_name_without_an_extension_gets_the_filters() {
        let d = FileDialog::new().add_filter("Session", &["audfrm"]);
        assert_eq!(
            d.with_extension(PathBuf::from("/x/Song")),
            PathBuf::from("/x/Song.audfrm")
        );
        assert_eq!(
            d.with_extension(PathBuf::from("/x/Song.sesx")),
            PathBuf::from("/x/Song.sesx")
        );
    }
}
