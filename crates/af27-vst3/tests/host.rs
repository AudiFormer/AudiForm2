// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! End-to-end: the test plug-in (crates/af27-vst3-testplug) is built into
//! a bundle, scanned, hosted in real host processes and played through
//! chains.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use af27_core::plugin::{new_inst, PluginData};
use af27_core::EffectSlot;
use af27_dsp::live::Live;
use af27_vst3::client::{Conn, Plug, STARTED};
use af27_vst3::live::VstEffect;
use af27_vst3::protocol::{ClassInfo, ScanResult};
use af27_vst3::registry::scan_bundle;
use af27_vst3::{build_live, manager};

/// Host processes and environment variables are shared: one test at a time.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_af27-plughost"))
}

/// Builds the test plug-in (in a target folder of its own, so it never
/// waits on the build running these tests) and wraps it in a bundle.
fn bundle() -> PathBuf {
    static B: OnceLock<PathBuf> = OnceLock::new();
    B.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join("testplug");
        let cargo = option_env!("CARGO").unwrap_or("cargo");
        let st = std::process::Command::new(cargo)
            .args(["build", "--offline", "-q", "-p", "af27-vst3-testplug"])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("cargo runs");
        assert!(st.success(), "the test plug-in builds");
        let lib = target.join("debug").join(if cfg!(windows) {
            "af27_vst3_testplug.dll"
        } else {
            "libaf27_vst3_testplug.so"
        });
        let b = target.join("bundles").join("AF2 Test.vst3");
        let arch = if cfg!(windows) {
            "x86_64-win"
        } else {
            "x86_64-linux"
        };
        let dir = b.join("Contents").join(arch);
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

fn scan() -> ScanResult {
    scan_bundle(
        &host(),
        &bundle(),
        Duration::from_secs(60),
        &AtomicBool::new(false),
    )
}

fn class(r: &ScanResult, name: &str) -> ClassInfo {
    r.classes
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("{name} in {r:?}"))
        .clone()
}

/// The manager set up with the test bundle in its registry.
fn setup() -> ScanResult {
    let r = scan();
    let m = manager();
    m.set_host_exe(host());
    m.registry().take_scan(&bundle(), r.clone());
    r
}

fn slot(cid: &str) -> EffectSlot {
    EffectSlot {
        effect: format!("vst3:{cid}"),
        values: Vec::new(),
        bypass: false,
        sidechain: None,
        data: Some(
            PluginData {
                inst: new_inst(),
                ..Default::default()
            }
            .to_json(),
        ),
        ext_id: None,
    }
}

fn impulse(n: usize, at: usize) -> Vec<f32> {
    let mut v = vec![0.0; n * 2];
    v[at * 2] = 1.0;
    v[at * 2 + 1] = 1.0;
    v
}

fn peak_at(v: &[f32]) -> (usize, f32) {
    let (i, x) = v
        .iter()
        .step_by(2)
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    (i, *x)
}

#[test]
fn a_scan_reads_both_plug_ins_and_a_missing_bundle_is_an_error() {
    let _g = serial();
    let r = scan();
    assert!(r.done && r.error.is_none(), "{r:?}");
    let gain = class(&r, "AF2 Test Gain");
    assert_eq!(gain.vendor, "AudiForm Tests", "vendor from the factory");
    assert_eq!(gain.latency, 32);
    assert_eq!(gain.params.len(), 3);
    assert_eq!(gain.params[0].title, "Gain");
    assert!(!gain.sidechain && !gain.instrument && gain.has_editor);
    assert!(class(&r, "AF2 Test Keyed").sidechain);
    let none = scan_bundle(
        &host(),
        Path::new("C:/nowhere/Nothing.vst3"),
        Duration::from_secs(20),
        &AtomicBool::new(false),
    );
    assert!(none.classes.is_empty());
    assert!(none.error.is_some());
}

#[test]
fn a_plug_in_that_crashes_on_start_is_named_with_a_note() {
    let _g = serial();
    std::env::set_var("AF2_TESTPLUG_CRASH_ON_START", "1");
    let r = scan();
    std::env::remove_var("AF2_TESTPLUG_CRASH_ON_START");
    assert!(!r.done);
    let gain = class(&r, "AF2 Test Gain");
    let note = gain.note.unwrap();
    assert!(note.contains("switching it on"), "{note}");
}

#[test]
fn processes_with_latency_gain_changes_and_long_blocks() {
    let _g = serial();
    let r = scan();
    let gain = class(&r, "AF2 Test Gain");
    let conn = Conn::spawn(&host()).unwrap();
    // Controls given at load count at once: 0.25 = half gain.
    let p = Plug::load(&conn, &bundle(), &gain.cid, 48_000, None, vec![(0, 0.25)]).unwrap();
    assert_eq!(p.info.latency, 32);
    let (mut out, mut bytes) = (Vec::new(), Vec::new());
    let t = Duration::from_secs(10);
    let ok = p
        .process(&impulse(256, 10), None, &[], &mut out, &mut bytes, t)
        .unwrap();
    assert!(ok);
    let (at, v) = peak_at(&out);
    assert_eq!(at, 42, "32 samples late");
    assert!((v - 0.5).abs() < 1e-6, "{v}");
    // A change applies within the block it comes with.
    p.process(
        &impulse(256, 10),
        None,
        &[(0, 1.0)],
        &mut out,
        &mut bytes,
        t,
    )
    .unwrap();
    assert!((peak_at(&out).1 - 2.0).abs() < 1e-6);
    // A block over the host's largest is split, seamlessly.
    let n = 20_000;
    let ramp: Vec<f32> = (0..n * 2).map(|i| (i / 2) as f32 * 1e-4).collect();
    p.process(&ramp, None, &[(0, 0.5)], &mut out, &mut bytes, t)
        .unwrap();
    for i in 1000..n {
        let want = (i - 32) as f32 * 1e-4;
        assert!((out[i * 2] - want).abs() < 1e-6, "frame {i}");
    }
    // Settings carry over into a new copy.
    p.process(&impulse(64, 0), None, &[(0, 0.75)], &mut out, &mut bytes, t)
        .unwrap();
    let state = p.state().unwrap();
    drop(p);
    let q = Plug::load(&conn, &bundle(), &gain.cid, 48_000, Some(state), vec![]).unwrap();
    q.process(&impulse(256, 10), None, &[], &mut out, &mut bytes, t)
        .unwrap();
    assert!((peak_at(&out).1 - 1.5).abs() < 1e-6, "restored 1.5x");
    // Two plug-ins in one process.
    let k = Plug::load(&conn, &bundle(), &gain.cid, 48_000, None, vec![(0, 0.5)]).unwrap();
    k.process(&impulse(256, 10), None, &[], &mut out, &mut bytes, t)
        .unwrap();
    assert!((peak_at(&out).1 - 1.0).abs() < 1e-6);
    assert_eq!(conn.count(), 2);
}

#[test]
fn a_sidechain_input_hears_the_key_and_reports_its_keyed_latency() {
    let _g = serial();
    let r = scan();
    let keyed = class(&r, "AF2 Test Keyed");
    let conn = Conn::spawn(&host()).unwrap();
    let p = Plug::load(&conn, &bundle(), &keyed.cid, 48_000, None, vec![]).unwrap();
    assert!(p.info.sidechain);
    assert_eq!(p.info.latency, 32);
    assert_eq!(p.info.keyed_latency, Some(64));
    let (mut out, mut bytes) = (Vec::new(), Vec::new());
    let t = Duration::from_secs(10);
    // No key: it hears its own input.
    p.process(&impulse(256, 10), None, &[], &mut out, &mut bytes, t)
        .unwrap();
    assert_eq!(peak_at(&out).0, 42);
    // A key: the key comes out, at the keyed latency.
    let silent = vec![0.0; 512];
    p.process(
        &silent,
        Some(&impulse(256, 20)),
        &[],
        &mut out,
        &mut bytes,
        t,
    )
    .unwrap();
    let (at, v) = peak_at(&out);
    assert_eq!((at, v), (84, 1.0));
}

#[test]
fn a_crash_leaves_the_chain_playing_dry_and_in_time() {
    let _g = serial();
    let r = setup();
    let gain = class(&r, "AF2 Test Gain");
    let s = slot(&gain.cid);
    let mut fx = VstEffect::new(&s, 48_000).expect("installed");
    fx.set_offline(true);
    assert_eq!(fx.latency(), 32, "offline: no buffer");
    let inst = fx.instance().unwrap().clone();
    assert!(inst.running(), "{:?}", inst.status());
    let n = 256;
    let mut run = |at: usize| {
        let mut l = vec![0.0f32; n];
        l[at] = 1.0;
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        let (i, v) = l
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap();
        (i, *v)
    };
    assert_eq!(run(10), (42, 1.0));
    inst.set_param(0, 0.25);
    assert_eq!(run(10), (42, 0.5), "half gain");
    // Crash it: the sound carries on dry, still 32 late.
    inst.set_param(1, 1.0);
    assert_eq!(run(10), (42, 1.0), "dry, in time");
    assert_eq!(run(100), (132, 1.0));
    let _ = inst.take_notices();
    assert!(matches!(inst.status(), af27_vst3::live::Status::Stopped(_)));
}

#[test]
fn live_plays_through_the_buffer_and_a_late_block_is_bypassed_not_killed() {
    let _g = serial();
    let r = setup();
    let gain = class(&r, "AF2 Test Gain");
    let s = slot(&gain.cid);
    let mut fx = build_live(&s, 48_000).unwrap();
    let key = s.plugin_data().inst;
    let inst = manager().get(&key).unwrap();
    let t = Instant::now();
    while !inst.running() && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(inst.running());
    let total = fx.latency();
    assert_eq!(total, manager().pipeline() + 32);
    // Play a steady signal in real time and check what comes out once
    // the buffer has filled: the gained input (0.5 -> 1x here, so a
    // value of 0.25 stays 0.25).
    let n = 256;
    let block = Duration::from_secs_f64(n as f64 / 48_000.0);
    let mut outs = Vec::new();
    for _ in 0..60 {
        let mut l = vec![0.25f32; n];
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        outs.push(l[n - 1]);
        std::thread::sleep(block);
    }
    assert!(
        outs[40..].iter().all(|v| (v - 0.25).abs() < 1e-6),
        "{outs:?}"
    );
    // Halve it: the change reaches the plug-in.
    inst.set_param(0, 0.25);
    for _ in 0..30 {
        let mut l = vec![0.25f32; n];
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        outs.push(l[n - 1]);
        std::thread::sleep(block);
    }
    assert!((outs.last().unwrap() - 0.125).abs() < 1e-6, "{outs:?}");
    // One block takes 6 s: meanwhile the dry sound plays, and then the
    // plug-in plays again, not killed.
    inst.set_param(2, 1.0);
    let t = Instant::now();
    let mut dry = 0;
    while t.elapsed() < Duration::from_secs(9) {
        let mut l = vec![0.25f32; n];
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        if (l[n - 1] - 0.25).abs() < 1e-6 {
            dry += 1;
        }
        std::thread::sleep(block);
    }
    assert!(dry > 100, "dry while late: {dry}");
    inst.set_param(2, 0.0);
    let mut last = 0.0;
    for _ in 0..80 {
        let mut l = vec![0.25f32; n];
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        last = l[n - 1];
        std::thread::sleep(block);
    }
    assert!(inst.running(), "{:?}", inst.status());
    assert!((last - 0.125).abs() < 1e-6, "processed again: {last}");
}

#[test]
fn slots_of_one_maker_share_a_process_and_missing_plug_ins_are_skipped() {
    let _g = serial();
    let r = setup();
    let gain = class(&r, "AF2 Test Gain");
    let keyed = class(&r, "AF2 Test Keyed");
    let before = STARTED.load(Ordering::SeqCst);
    let a = build_live(&slot(&gain.cid), 44_100).unwrap();
    let b = build_live(&slot(&keyed.cid), 44_100).unwrap();
    let t = Instant::now();
    let all_running = || {
        manager()
            .instances()
            .iter()
            .filter(|i| i.rate == 44_100)
            .all(|i| i.running())
    };
    while !all_running() && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(all_running());
    assert_eq!(
        STARTED.load(Ordering::SeqCst) - before,
        1,
        "one process for the maker"
    );
    drop((a, b));
    assert!(build_live(&slot("00112233445566778899AABBCCDDEEFF"), 48_000).is_none());
}
