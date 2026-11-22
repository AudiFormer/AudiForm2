// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The manifest published with the engine-v1.0.0 pre-release, as
//! downloaded: it must read, speak protocol 2 and name a build for each
//! platform the app ships on.

use audiform_separate_client::Manifest;

#[test]
fn the_published_engine_manifest_reads() {
    let m = Manifest::parse(include_bytes!("data/engine-v1.0.0-manifest.json")).unwrap();
    assert_eq!(m.engine, "1.0.0");
    assert_eq!(m.protocol, 2);
    for p in ["windows-x64", "linux-x64"] {
        let a = &m.assets[p];
        assert!(a.file.starts_with("audiform2-separate-"), "{}", a.file);
        assert_eq!(a.sha256.len(), 64);
    }
    #[cfg(windows)]
    assert_eq!(
        m.asset().unwrap().file,
        "audiform2-separate-windows-x64.zip"
    );
}
