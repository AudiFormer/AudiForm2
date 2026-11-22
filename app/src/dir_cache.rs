// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The names of the saved presets in a folder, read again only every few
//! seconds (or at once after a save or delete here) rather than every
//! frame an effect window or rack is drawn: listing a folder is a trip to
//! the disk, slow on a network home folder.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a listing is trusted (to notice files added from outside).
const FRESH: Duration = Duration::from_secs(2);

thread_local! {
    static LISTS: RefCell<HashMap<PathBuf, (Instant, Vec<String>)>> = RefCell::new(HashMap::new());
}

/// The stems of the `.json` files in `dir` (in no order; empty when the
/// folder does not exist).
pub fn json_stems(dir: &Path) -> Vec<String> {
    LISTS.with(|l| {
        let mut l = l.borrow_mut();
        if let Some((at, v)) = l.get(dir) {
            if at.elapsed() < FRESH {
                return v.clone();
            }
        }
        let v: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .collect();
        l.insert(dir.to_path_buf(), (Instant::now(), v.clone()));
        v
    })
}

/// A file in `dir` was saved or deleted: the next listing reads it again.
pub fn forget(dir: &Path) {
    LISTS.with(|l| {
        l.borrow_mut().remove(dir);
    });
}
