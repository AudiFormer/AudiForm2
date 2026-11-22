// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Opens every file given (or every file in a folder given) the way the
//! app does and prints one line each: rate, channels, length, notices,
//! markers and tags. A quick check of which formats open.
//!
//! `cargo run -p af2-io --example open_report -- <files or folder>`

use std::path::PathBuf;

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    for a in std::env::args().skip(1) {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
                .unwrap_or_default();
            v.sort();
            files.extend(v.into_iter().filter(|p| p.is_file()));
        } else {
            files.push(p);
        }
    }
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        match af2_io::open_file(&f) {
            Ok(o) => {
                let tags: Vec<String> = o
                    .info
                    .tags
                    .rows()
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                println!(
                    "OK   {name:<24} {} Hz, {} ch, {:.3} s, peak {:.2}{}{}{}",
                    o.buf.rate,
                    o.buf.num_channels(),
                    o.buf.duration_secs(),
                    o.buf.peak(),
                    o.notice().map(|n| format!(" | {n}")).unwrap_or_default(),
                    if o.info.markers.is_empty() {
                        String::new()
                    } else {
                        format!(" | {} markers", o.info.markers.len())
                    },
                    if tags.is_empty() {
                        String::new()
                    } else {
                        format!(" | {}", tags.join(", "))
                    },
                );
            }
            Err(e) => println!("FAIL {name:<24} {e}"),
        }
    }
}
