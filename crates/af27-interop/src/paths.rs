// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Path helpers shared by the session formats: tidying `..` out of a
//! path, comparing two spellings of one file, paths relative to a session
//! folder, and saving a file so a crash never leaves half of it.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Removes `.` and `..` from a path without touching the disk.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    out.iter().map(|c| c.as_os_str()).collect()
}

/// A path as text for comparing: `/` separators, no `.`/`..`, and on
/// Windows in lower case (its file names ignore case).
fn key(p: &Path) -> String {
    let s = normalize(Path::new(&p.to_string_lossy().replace('\\', "/")))
        .to_string_lossy()
        .replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Whether two paths name the same file as written (no disk access).
pub fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// `path` relative to the folder `dir`, with `/` separators so a session
/// saved on Windows opens elsewhere. None when either is not absolute or
/// they are on different drives.
pub fn relative(path: &Path, dir: &Path) -> Option<String> {
    if !path.is_absolute() || !dir.is_absolute() {
        return None;
    }
    let p = normalize(path);
    let d = normalize(dir);
    let pc: Vec<Component> = p.components().collect();
    let dc: Vec<Component> = d.components().collect();
    let same_part = |a: &Component, b: &Component| {
        let (a, b) = (
            a.as_os_str().to_string_lossy(),
            b.as_os_str().to_string_lossy(),
        );
        if cfg!(windows) {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    };
    // Same drive (or root) first.
    match (pc.first(), dc.first()) {
        (Some(a), Some(b)) if same_part(a, b) => {}
        _ => return None,
    }
    let common = pc
        .iter()
        .zip(&dc)
        .take_while(|(a, b)| same_part(a, b))
        .count();
    let mut parts: Vec<String> = vec!["..".into(); dc.len() - common];
    parts.extend(
        pc[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().to_string()),
    );
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Writes `bytes` to `path` through a temporary file in the same folder,
/// flushed to disk before it replaces the old file, so a crash or a full
/// disk mid-save leaves the old file as it was. A failed save removes
/// the temporary file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let fail = |e: std::io::Error| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not save {}: {e}", path.display())
    };
    let mut f = std::fs::File::create(&tmp).map_err(fail)?;
    f.write_all(bytes).map_err(fail)?;
    f.sync_all().map_err(fail)?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_come_out() {
        assert_eq!(
            normalize(Path::new("/show/412/../413/./host.wav")),
            PathBuf::from("/show/413/host.wav")
        );
        assert_eq!(normalize(Path::new("a/../../b")), PathBuf::from("../b"));
    }

    #[test]
    fn relative_paths_use_slashes_and_climb() {
        let root = std::env::temp_dir();
        let dir = root.join("Show").join("412");
        assert_eq!(
            relative(&dir.join("Audio").join("host.wav"), &dir).as_deref(),
            Some("Audio/host.wav")
        );
        assert_eq!(
            relative(&root.join("Music").join("bed.wav"), &dir).as_deref(),
            Some("../../Music/bed.wav")
        );
        assert_eq!(relative(Path::new("host.wav"), &dir), None);
        #[cfg(windows)]
        {
            assert_eq!(
                relative(Path::new(r"D:\Music\bed.wav"), Path::new(r"C:\Show")),
                None,
                "another drive has no relative path"
            );
            assert!(same(
                Path::new(r"C:\Show\A.wav"),
                Path::new("c:/show/x/../a.wav")
            ));
        }
    }

    #[test]
    fn saving_twice_replaces_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("af27-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Show.audfrm");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(names.len(), 1, "only the session file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
