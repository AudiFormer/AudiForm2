// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The scan process: opens one bundle, names its plug-ins, then starts
//! each to read it in full. Every step is printed as it happens, so the
//! app can tell what was found even if a plug-in takes the process down.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use super::module::Module;
use super::plugin::Plugin;
use crate::protocol::{named_instrument, scan_line, ClassInfo, ScanLine, STAGE_PREFIX};

fn say(line: &ScanLine) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{}", scan_line(line));
    let _ = o.flush();
}

fn stage(what: &str) {
    let mut e = std::io::stderr().lock();
    let _ = writeln!(e, "{STAGE_PREFIX}{what}");
    let _ = e.flush();
}

pub fn scan(bundle: &Path) -> i32 {
    stage("loading its file");
    let module = match Module::open(bundle) {
        Ok(m) => Arc::new(m),
        Err(message) => {
            say(&ScanLine::Error { message });
            say(&ScanLine::Done);
            crate::exit_now(0);
        }
    };
    let classes = module.audio_classes();
    let named: Vec<ClassInfo> = classes
        .iter()
        .map(|c| ClassInfo {
            cid: c.hex(),
            name: c.name.clone(),
            vendor: c.vendor.clone(),
            version: c.version.clone(),
            sub_categories: c.sub_categories.clone(),
            instrument: named_instrument(&c.sub_categories),
            ..Default::default()
        })
        .collect();
    say(&ScanLine::Found {
        classes: named.clone(),
    });
    let mut keep = Vec::new();
    for (c, info) in classes.iter().zip(named) {
        match Plugin::load(&module, &c.cid, 48_000.0, None, &[], &stage) {
            Ok((plugin, audio, loaded)) => {
                let class = ClassInfo {
                    params: loaded.params,
                    latency: loaded.latency,
                    has_editor: loaded.has_editor,
                    instrument: info.instrument || loaded.instrument,
                    sidechain: loaded.sidechain,
                    ..info
                };
                say(&ScanLine::Read { class });
                // Never closed: closing some plug-ins before the process
                // ends crashes in them.
                keep.push((plugin, audio));
            }
            Err(error) => say(&ScanLine::Failed {
                cid: info.cid,
                error,
            }),
        }
    }
    say(&ScanLine::Done);
    std::mem::forget(keep);
    std::mem::forget(module);
    crate::exit_now(0)
}
