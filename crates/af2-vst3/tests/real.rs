// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Against real installed plug-ins (ignored by default):
//! `AF2_REAL_VST3="C:\...\A.vst3;C:\...\B.vst3" cargo test -p af2-vst3 --test real -- --ignored --nocapture`
//! Each is scanned, started, fed noise, asked for its settings, restarted
//! with them, and has its window opened and closed.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use af2_vst3::client::{Conn, Plug};
use af2_vst3::protocol::Request;
use af2_vst3::registry::scan_bundle;

#[test]
#[ignore]
fn real_plugins_scan_process_keep_settings_and_open_windows() {
    let Some(list) = std::env::var_os("AF2_REAL_VST3") else {
        return;
    };
    let host = PathBuf::from(env!("CARGO_BIN_EXE_af2-plughost"));
    let editors = std::env::var_os("AF2_REAL_EDITORS").is_some();
    for b in list.to_string_lossy().split(';').filter(|s| !s.is_empty()) {
        let b = PathBuf::from(b);
        let r = scan_bundle(&host, &b, Duration::from_secs(60), &AtomicBool::new(false));
        assert!(r.error.is_none(), "{}: {:?}", b.display(), r.error);
        for c in r.classes.iter().filter(|c| !c.instrument) {
            let conn = Conn::spawn(&host).unwrap();
            let p = Plug::load(&conn, &b, &c.cid, 48_000, None, vec![]).unwrap();
            let n = 4096;
            let mut seed = 7u32;
            let noise: Vec<f32> = (0..n * 2)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 - 0.5) * 0.5
                })
                .collect();
            let (mut out, mut bytes) = (Vec::new(), Vec::new());
            let mut energy = 0.0f64;
            let t = std::time::Instant::now();
            for _ in 0..24 {
                let ok = p
                    .process(
                        &noise,
                        None,
                        &[],
                        &mut out,
                        &mut bytes,
                        Duration::from_secs(10),
                    )
                    .unwrap();
                assert!(ok, "{} ran", c.name);
                assert!(out.iter().all(|v| v.is_finite()), "{} finite", c.name);
                energy += out.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
            }
            let per_block = t.elapsed() / 24;
            let state = p.state().unwrap();
            let q = Plug::load(&conn, &b, &c.cid, 48_000, Some(state.clone()), vec![]).unwrap();
            q.process(
                &noise,
                None,
                &[],
                &mut out,
                &mut bytes,
                Duration::from_secs(10),
            )
            .unwrap();
            let mut window = "not tried".to_string();
            if editors && c.has_editor {
                window = match p.request(&Request::OpenEditor {
                    title: c.name.clone(),
                    owner: 0,
                    bypassed: false,
                }) {
                    Ok(_) => {
                        std::thread::sleep(Duration::from_millis(1500));
                        p.request(&Request::CloseEditor).unwrap();
                        "opened and closed".into()
                    }
                    Err(e) => format!("did not open: {e}"),
                };
            }
            eprintln!(
                "{} ({}): latency {}, {} controls, sidechain {}, state {} bytes, energy {:.1}, {:?} per 4096-frame block, window {}",
                c.name,
                c.vendor,
                p.info.latency,
                p.info.params.len(),
                p.info.sidechain,
                state.len() * 3 / 4,
                energy,
                per_block,
                window
            );
        }
    }
}
