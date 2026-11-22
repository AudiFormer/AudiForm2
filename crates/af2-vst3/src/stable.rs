// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! One fixed place plug-in host and scan processes run from.
//!
//! Windows asks "Allow access to the network?" the first time a program
//! listens for connections, and it remembers the answer (and any firewall
//! rule) by the program's path. AudiForm 2 itself never listens: it talks
//! to its hosts through their stdin and stdout pipes. But plug-ins do (a
//! licence check, or plug-ins of one maker talking to each other), and
//! they run inside a host process. If hosts ran from the app's own file,
//! every copy of the app (each test build with a new name) would be a new
//! path, and the question would come back with each one.
//!
//! So hosts run from a copy of the app at one fixed path,
//! `%LOCALAPPDATA%\AudiForm2\PlugInHost\AudiForm2 Plug-in Host.exe`,
//! refreshed whenever the running app differs from it. One answer (or one
//! firewall rule, from Settings) covers every build from then on.

use std::path::{Path, PathBuf};

/// The file name of the fixed host copy.
pub const HOST_NAME: &str = "AudiForm2 Plug-in Host.exe";

/// Where the fixed host copy lives (None where there is no such place,
/// as off Windows).
pub fn stable_host_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("AudiForm2")
            .join("PlugInHost")
            .join(HOST_NAME),
    )
}

fn same_file_contents(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Makes `dst` a copy of `src` and returns it; None when it cannot (the
/// caller then runs `src` itself).
///
/// A copy still running (hosts of another AudiForm 2 open at the same
/// time) cannot be overwritten, but Windows lets a running program's file
/// be renamed: it moves aside to `<name>.old-<n>`, and is deleted on a
/// later start once nothing runs from it.
pub fn refresh(src: &Path, dst: &Path) -> Option<PathBuf> {
    let dir = dst.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    // Old copies set aside earlier: gone once nothing runs from them.
    let stem = dst.file_name()?.to_string_lossy().into_owned();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&format!("{stem}.old-")) || n.starts_with(&format!("{stem}.new-")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if same_file_contents(src, dst) {
        return Some(dst.to_path_buf());
    }
    let tmp = dir.join(format!("{stem}.new-{}", std::process::id()));
    std::fs::copy(src, &tmp).ok()?;
    if dst.exists() && std::fs::remove_file(dst).is_err() {
        // In use: move it aside.
        let mut n = 0u32;
        loop {
            let old = dir.join(format!("{stem}.old-{n}"));
            if !old.exists() {
                if std::fs::rename(dst, &old).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                    return None;
                }
                break;
            }
            n += 1;
            if n > 1000 {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
        }
    }
    if std::fs::rename(&tmp, dst).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(dst.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_refreshes_and_keeps() {
        let dir = std::env::temp_dir().join(format!("af2-stable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("app.exe");
        let dst = dir.join("host").join("Host.exe");
        std::fs::write(&src, b"build one").unwrap();
        assert_eq!(refresh(&src, &dst), Some(dst.clone()));
        assert_eq!(std::fs::read(&dst).unwrap(), b"build one");
        // Same contents: kept as is.
        assert_eq!(refresh(&src, &dst), Some(dst.clone()));
        // A new build: refreshed at the same path.
        std::fs::write(&src, b"build two, longer").unwrap();
        assert_eq!(refresh(&src, &dst), Some(dst.clone()));
        assert_eq!(std::fs::read(&dst).unwrap(), b"build two, longer");
        let left: Vec<_> = std::fs::read_dir(dst.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "no temporary files left: {left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
