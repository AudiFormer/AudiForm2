// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Plug-ins inside the engine's mix: an export (an offline render) through
//! a session whose tracks hold the test plug-in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use af27_core::plugin::{new_inst, PluginData};
use af27_core::session::{Clip, MediaItem};
use af27_core::{AudioBuffer, EffectSlot, MediaId, Session, TrackId, TrackKind};
use af27_vst3::registry::scan_bundle;
use af27_vst3::{build_live, manager};

const RATE: u32 = 48_000;

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_af27-plughost"))
}

/// Builds the test plug-in into a bundle (as tests/host.rs does, in a
/// target folder of its own).
fn bundle() -> PathBuf {
    static B: OnceLock<PathBuf> = OnceLock::new();
    B.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join("testplug-engine");
        let cargo = option_env!("CARGO").unwrap_or("cargo");
        let st = std::process::Command::new(cargo)
            .args(["build", "--offline", "-q", "-p", "af27-vst3-testplug"])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("cargo runs");
        assert!(st.success());
        let lib = target.join("debug").join(if cfg!(windows) {
            "af27_vst3_testplug.dll"
        } else {
            "libaf27_vst3_testplug.so"
        });
        let b = target.join("bundles").join("AF2 Test.vst3");
        let dir = b.join("Contents").join(if cfg!(windows) {
            "x86_64-win"
        } else {
            "x86_64-linux"
        });
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) {
            "AF2 Test.vst3"
        } else {
            "AF2 Test.so"
        };
        std::fs::copy(&lib, dir.join(name)).unwrap();
        b
    })
    .clone()
}

/// The test plug-ins' class ids: (gain, keyed).
fn setup() -> (String, String) {
    let r = scan_bundle(
        &host(),
        &bundle(),
        Duration::from_secs(60),
        &AtomicBool::new(false),
    );
    let id = |n: &str| r.classes.iter().find(|c| c.name == n).unwrap().cid.clone();
    let ids = (id("AF2 Test Gain"), id("AF2 Test Keyed"));
    manager().set_host_exe(host());
    manager().registry().take_scan(&bundle(), r);
    af27_dsp::live::register_factory(build_live);
    ids
}

fn plugin(cid: &str, params: &[(u32, f64)], key: Option<TrackId>) -> EffectSlot {
    let mut d = PluginData {
        inst: new_inst(),
        ..Default::default()
    };
    for &(id, v) in params {
        d.set_param(id, v);
    }
    EffectSlot {
        position: None,
        effect: format!("vst3:{cid}"),
        values: Vec::new(),
        bypass: false,
        sidechain: key,
        data: Some(d.to_json()),
        ext_id: None,
    }
}

struct Fixture {
    s: Session,
    bufs: HashMap<MediaId, Arc<AudioBuffer>>,
}

impl Fixture {
    /// A track playing one click at `at` seconds (0 for a silent track).
    fn track(&mut self, at: f64, level: f32) -> TrackId {
        let id = self.s.add_track_of(TrackKind::Audio);
        if level == 0.0 {
            return id;
        }
        let m = self.s.alloc_id();
        let mut ch = vec![0.0f32; RATE as usize];
        ch[(at * RATE as f64) as usize] = level;
        self.s.media.push(MediaItem {
            id: m,
            name: "m".into(),
            path: None,
            duration: 1.0,
            rate: RATE,
            channels: 1,
            ext_id: None,
        });
        self.bufs.insert(
            m,
            Arc::new(AudioBuffer {
                rate: RATE,
                channels: vec![ch],
            }),
        );
        let cid = self.s.alloc_id();
        self.s.track_mut(id).unwrap().clips.push(Clip {
            props: Default::default(),
            id: cid,
            media: m,
            name: "c".into(),
            start: 0.0,
            offset: 0.0,
            length: 1.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            ext_id: None,
        });
        id
    }

    fn render(&self) -> Vec<f32> {
        let bufs = self.bufs.clone();
        let p = af27_engine::mix_program(&self.s, &move |m| bufs.get(&m).cloned(), RATE);
        af27_engine::render(p, RATE, 0.0, 1.0).channels[0].clone()
    }
}

fn clicks(x: &[f32]) -> Vec<(usize, f32)> {
    x.iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > 1e-4)
        .map(|(i, v)| (i, *v))
        .collect()
}

#[test]
fn an_export_runs_plug_ins_in_their_own_copies_with_sidechain_keys() {
    let (gain, keyed) = setup();
    let mut f = Fixture {
        s: Session::new("vst", RATE, 0),
        bufs: HashMap::new(),
    };
    let a = f.track(0.1, 0.5);
    let dry = clicks(&f.render());
    assert_eq!(dry.len(), 1, "{dry:?}");
    let (at, level) = dry[0];
    // Half gain, on time: the plug-in's own copy is 32 samples late and
    // the mix's delay compensation reads the track that far ahead.
    f.s.track_mut(a)
        .unwrap()
        .chain
        .push(plugin(&gain, &[(0, 0.25)], None));
    let wet = clicks(&f.render());
    assert_eq!(wet.len(), 1, "{wet:?}");
    assert_eq!(wet[0].0, at);
    assert!((wet[0].1 - level * 0.5).abs() < 1e-6, "{wet:?}");
    // A silent track keyed by track A plays A's sound (the test plug-in
    // passes the key through), at its keyed latency.
    let b = f.track(0.0, 0.0);
    f.s.track_mut(b)
        .unwrap()
        .chain
        .push(plugin(&keyed, &[], Some(a)));
    let both = clicks(&f.render());
    // The key is A after its chain (half gain); B's keyed latency (64) is
    // compensated too, so A and B land together, on time.
    assert_eq!(both.len(), 1, "{both:?}");
    assert_eq!(both[0].0, at, "A and B in step");
    assert!((both[0].1 - level).abs() < 1e-6, "{both:?}");
    // A plug-in that is not installed stays in the chain and is skipped.
    f.s.track_mut(a)
        .unwrap()
        .chain
        .insert(0, plugin("00112233445566778899AABBCCDDEEFF", &[], None));
    let skipped = clicks(&f.render());
    assert_eq!(skipped[0].0, at);
}
