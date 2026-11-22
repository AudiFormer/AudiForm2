// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! `.vstpreset` files, shared with every VST3 host.
//!
//! The file: `VST3`, an `i32` version (1), the class id as 32 ASCII hex
//! digits, the `i64` offset of the chunk list, the chunks, then the list:
//! `List`, an `i32` count and per chunk a 4-byte id, an `i64` offset and an
//! `i64` size. `Comp` is the component's settings, `Cont` the
//! controller's.

use std::path::{Path, PathBuf};

use crate::protocol::{cid_other_order, join_state, split_state};

/// Where presets live: the user's folder first, then the shared one.
pub fn folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(h) = std::env::var_os("USERPROFILE") {
            out.push(PathBuf::from(h).join("Documents").join("VST3 Presets"));
        }
        if let Some(p) = std::env::var_os("PROGRAMDATA") {
            out.push(PathBuf::from(p).join("VST3 Presets"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = std::env::var_os("HOME") {
            out.push(PathBuf::from(h).join("Library/Audio/Presets"));
        }
        out.push(PathBuf::from("/Library/Audio/Presets"));
    } else {
        if let Some(h) = std::env::var_os("HOME") {
            out.push(PathBuf::from(h).join(".vst3/presets"));
        }
        out.push(PathBuf::from("/usr/share/vst3/presets"));
        out.push(PathBuf::from("/usr/local/share/vst3/presets"));
    }
    out
}

/// Where a new preset of a plug-in is saved.
pub fn user_folder(vendor: &str, name: &str) -> Option<PathBuf> {
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            .collect::<String>()
            .trim()
            .to_string()
    };
    folders()
        .into_iter()
        .next()
        .map(|f| f.join(clean(vendor)).join(clean(name)))
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The class id and settings blob (see [`join_state`]) in a preset file's
/// bytes.
pub fn parse(b: &[u8]) -> Result<(String, Vec<u8>), String> {
    let bad = || "This is not a VST3 preset.".to_string();
    if b.get(..4) != Some(b"VST3") {
        return Err(bad());
    }
    let cid = std::str::from_utf8(b.get(8..40).ok_or_else(bad)?)
        .map_err(|_| bad())?
        .to_ascii_uppercase();
    let list = usize::try_from(i64_at(b, 40).ok_or_else(bad)?).map_err(|_| bad())?;
    if b.get(list..list + 4) != Some(b"List") {
        return Err(bad());
    }
    let n = i32_at(b, list + 4).ok_or_else(bad)?.max(0) as usize;
    let (mut comp, mut ctrl): (&[u8], &[u8]) = (&[], &[]);
    for k in 0..n {
        let at = list + 8 + k * 20;
        let id = b.get(at..at + 4).ok_or_else(bad)?;
        let off = usize::try_from(i64_at(b, at + 4).ok_or_else(bad)?).map_err(|_| bad())?;
        let size = usize::try_from(i64_at(b, at + 12).ok_or_else(bad)?).map_err(|_| bad())?;
        let data = b
            .get(off..off.checked_add(size).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        match id {
            b"Comp" => comp = data,
            b"Cont" => ctrl = data,
            _ => {}
        }
    }
    Ok((cid, join_state(comp, ctrl)))
}

/// A preset file's bytes for plug-in `cid` with settings `blob`.
pub fn build(cid: &str, blob: &[u8]) -> Vec<u8> {
    let (comp, ctrl) = split_state(blob).unwrap_or((blob, &[]));
    let mut out = Vec::new();
    out.extend_from_slice(b"VST3");
    out.extend_from_slice(&1i32.to_le_bytes());
    let mut id = cid.as_bytes().to_vec();
    id.resize(32, b'0');
    out.extend_from_slice(&id);
    let head = out.len() + 8;
    let list = head + comp.len() + ctrl.len();
    out.extend_from_slice(&(list as i64).to_le_bytes());
    out.extend_from_slice(comp);
    out.extend_from_slice(ctrl);
    let chunks: Vec<(&[u8; 4], usize, usize)> = {
        let mut v = vec![(b"Comp", head, comp.len())];
        if !ctrl.is_empty() {
            v.push((b"Cont", head + comp.len(), ctrl.len()));
        }
        v
    };
    out.extend_from_slice(b"List");
    out.extend_from_slice(&(chunks.len() as i32).to_le_bytes());
    for (id, off, size) in chunks {
        out.extend_from_slice(id);
        out.extend_from_slice(&(off as i64).to_le_bytes());
        out.extend_from_slice(&(size as i64).to_le_bytes());
    }
    out
}

/// Reads a preset file: (class id, settings blob).
pub fn read(path: &Path) -> Result<(String, Vec<u8>), String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    parse(&b)
}

pub fn save(path: &Path, cid: &str, blob: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, build(cid, blob)).map_err(|e| e.to_string())
}

/// Whether a preset's class id is plug-in `cid` (either byte order).
pub fn matches(preset_cid: &str, cid: &str) -> bool {
    preset_cid.eq_ignore_ascii_case(cid)
        || cid_other_order(cid).is_some_and(|o| o.eq_ignore_ascii_case(preset_cid))
}

/// Every preset of plug-in `cid` in the preset folders (five levels
/// deep), sorted by name.
pub fn list(cid: &str) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: usize, cid: &str, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, cid, out);
                }
            } else if p
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("vstpreset"))
            {
                // Only the header is needed to tell whose it is.
                let ok = std::fs::File::open(&p).ok().and_then(|mut f| {
                    use std::io::Read;
                    let mut head = [0u8; 40];
                    f.read_exact(&mut head).ok()?;
                    let id = std::str::from_utf8(&head[8..40]).ok()?.to_string();
                    Some(&head[..4] == b"VST3" && matches(&id, cid))
                });
                if ok == Some(true) {
                    out.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    for f in folders() {
        walk(&f, 5, cid, &mut out);
    }
    out.sort_by_key(|p| {
        p.file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_and_match_either_byte_order() {
        let cid = "0123456789ABCDEF0011223344556677";
        let blob = join_state(b"component part", b"ctrl");
        let bytes = build(cid, &blob);
        assert_eq!(&bytes[..4], b"VST3");
        let (id, back) = parse(&bytes).unwrap();
        assert_eq!(id, cid);
        assert_eq!(back, blob);
        // No controller part: no Cont chunk, and it reads back empty.
        let solo = join_state(b"only", b"");
        assert_eq!(parse(&build(cid, &solo)).unwrap().1, solo);
        assert!(matches(&cid_other_order(cid).unwrap(), cid));
        assert!(!matches("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF", cid));
        assert!(parse(b"nope").is_err());
        let mut cut = bytes.clone();
        cut.truncate(50);
        assert!(parse(&cut).is_err());
        let dir = std::env::temp_dir().join(format!("af2-preset-{}", std::process::id()));
        let p = dir.join("x.vstpreset");
        save(&p, cid, &blob).unwrap();
        assert_eq!(read(&p).unwrap(), (cid.to_string(), blob));
        let _ = std::fs::remove_dir_all(dir);
    }
}
