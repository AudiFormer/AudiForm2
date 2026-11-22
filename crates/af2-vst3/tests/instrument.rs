// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! VST3 instruments end to end: the test plug-in's "AF2 Test Synth" (a
//! sine per held note, 16 samples late, taking the sustain pedal, pitch
//! bend and mod wheel by its MIDI mapping) scanned, played note by note,
//! from a MIDI track in an export, and from the keys.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use af2_core::midi::{ControlEvent, MidiClip, MidiNote};
use af2_core::plugin::{new_inst, PluginData};
use af2_core::session::{Clip, MediaItem};
use af2_core::{AudioBuffer, EffectSlot, Session, TrackKind};
use af2_dsp::instrument::{Instrument, NoteEvent, Timed, Use};
use af2_vst3::live::{Status, VstInstrument};
use af2_vst3::protocol::ClassInfo;
use af2_vst3::registry::scan_bundle;
use af2_vst3::{build_instrument, build_live, manager};

const RATE: u32 = 48_000;

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_af2-plughost"))
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
            .join("testplug-inst");
        let cargo = option_env!("CARGO").unwrap_or("cargo");
        let st = std::process::Command::new(cargo)
            .args(["build", "--offline", "-q", "-p", "af2-vst3-testplug"])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("cargo runs");
        assert!(st.success(), "the test plug-in builds");
        let lib = target.join("debug").join(if cfg!(windows) {
            "af2_vst3_testplug.dll"
        } else {
            "libaf2_vst3_testplug.so"
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

/// The synth's class, with the manager and the engine set up.
fn setup() -> ClassInfo {
    static C: OnceLock<ClassInfo> = OnceLock::new();
    C.get_or_init(|| {
        let r = scan_bundle(
            &host(),
            &bundle(),
            Duration::from_secs(60),
            &AtomicBool::new(false),
        );
        assert!(r.done && r.error.is_none(), "{r:?}");
        let c = r
            .classes
            .iter()
            .find(|c| c.name == "AF2 Test Synth")
            .unwrap_or_else(|| panic!("the synth in {r:?}"))
            .clone();
        manager().set_host_exe(host());
        manager().registry().take_scan(&bundle(), r);
        af2_dsp::live::register_factory(build_live);
        af2_dsp::instrument::register_instrument_factory(build_instrument);
        c
    })
    .clone()
}

fn slot(cid: &str) -> EffectSlot {
    EffectSlot {
        position: None,
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

fn on(offset: u32, pitch: u8) -> Timed {
    Timed {
        offset,
        event: NoteEvent::On {
            pitch,
            velocity: 0.5,
            channel: 0,
            tuning: 0.0,
        },
    }
}

fn off(offset: u32, pitch: u8) -> Timed {
    Timed {
        offset,
        event: NoteEvent::Off { pitch, channel: 0 },
    }
}

fn cc(offset: u32, controller: u8, value: f32) -> Timed {
    Timed {
        offset,
        event: NoteEvent::Control {
            controller,
            value,
            channel: 0,
        },
    }
}

fn block(i: &mut VstInstrument, events: &[Timed], n: usize) -> Vec<f32> {
    let mut l = vec![1.0f32; n];
    let mut r = vec![1.0f32; n];
    i.render(events, &mut l, &mut r);
    assert_eq!(l, r, "the same on both sides");
    l
}

fn first_sound(x: &[f32]) -> Option<usize> {
    x.iter().position(|v| v.abs() > 1e-4)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Sign changes per second.
fn crossings(x: &[f32]) -> f64 {
    let n = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    n as f64 * RATE as f64 / x.len() as f64
}

#[test]
fn a_scan_names_the_synth_an_instrument_that_takes_the_pedal() {
    let _g = serial();
    let c = setup();
    assert!(c.instrument, "{c:?}");
    assert_eq!(c.sub_categories, "Instrument|Synth");
    assert_eq!(c.vendor, "AudiForm Tests");
    assert_eq!(c.latency, 16);
    let found = manager().find(&c.cid).unwrap();
    assert_eq!(found.instrument_kind().as_deref(), Some("Synth"));
    assert!(manager()
        .registry()
        .instruments()
        .iter()
        .any(|f| f.info.cid == c.cid));
    // Running, it says which controllers its MIDI mapping takes.
    let mut i = VstInstrument::new(&slot(&c.cid), RATE, Use::Playback).unwrap();
    i.set_offline(true);
    assert!(i.takes(64) && i.takes(129) && i.takes(1));
    assert!(!i.takes(7), "volume is not mapped");
}

#[test]
fn notes_sound_on_their_frame_end_and_the_pedal_holds_them() {
    let _g = serial();
    let c = setup();
    let mut i = VstInstrument::new(&slot(&c.cid), RATE, Use::Playback).unwrap();
    i.set_offline(true);
    assert_eq!(i.latency(), 16, "offline: its own latency only");
    let n = 1024;
    // Silence first, written over whatever the buffers held.
    assert_eq!(peak(&block(&mut i, &[], n)), 0.0);
    // A4 at frame 100: heard 16 frames later (a sine starts at 0).
    let b = block(&mut i, &[on(100, 69)], n);
    let at = first_sound(&b).unwrap();
    assert!((116..=118).contains(&at), "{at}");
    assert!((peak(&b) - 0.5).abs() < 0.01, "velocity 0.5: {}", peak(&b));
    let b = block(&mut i, &[], 4800);
    assert!((crossings(&b) - 880.0).abs() < 15.0, "{}", crossings(&b));
    // Pitch bend all the way up: two semitones.
    let b = block(&mut i, &[cc(0, 129, 1.0)], 4800);
    let want = 2.0 * 440.0 * 2f64.powf(2.0 / 12.0);
    assert!((crossings(&b[64..]) - want).abs() < 15.0, "{}", crossings(&b));
    block(&mut i, &[cc(0, 129, 0.5)], n);
    // Let go at frame 10: silent from frame 26.
    let b = block(&mut i, &[off(10, 69)], n);
    assert!(peak(&b[..20]) > 0.1);
    assert_eq!(peak(&b[30..]), 0.0, "ended");
    // The pedal down: a note let go keeps sounding until it comes up.
    let b = block(&mut i, &[cc(0, 64, 1.0), on(0, 60), off(200, 60)], n);
    assert!(peak(&b[400..]) > 0.1, "held by the pedal");
    let b = block(&mut i, &[], n);
    assert!(peak(&b) > 0.1, "still held");
    let b = block(&mut i, &[cc(500, 64, 0.0)], n);
    assert!(peak(&b[..500]) > 0.1);
    assert_eq!(peak(&b[530..]), 0.0, "the pedal came up");
}

#[test]
fn a_crash_leaves_silence_and_says_so() {
    let _g = serial();
    let c = setup();
    let mut i = VstInstrument::new(&slot(&c.cid), RATE, Use::Playback).unwrap();
    i.set_offline(true);
    let inst = i.instance().unwrap().clone();
    assert!(inst.running());
    block(&mut i, &[on(0, 69)], 256);
    // Key 0 crashes it: from then on the instrument is silent, not stuck.
    let b = block(&mut i, &[on(0, 0)], 256);
    assert!(b.iter().all(|v| v.is_finite()));
    let b = block(&mut i, &[], 256);
    assert_eq!(peak(&b), 0.0);
    let _ = inst.take_notices();
    assert!(matches!(inst.status(), Status::Stopped(_)), "{:?}", inst.status());
}

/// A session with a MIDI track playing the synth.
fn session(cid: &str, notes: &[(u8, f64, f64)], controls: &[(f64, u8, f32)]) -> Session {
    let mut s = Session::new("inst", RATE, 0);
    let t = s.add_track_of(TrackKind::Midi);
    let id = s.alloc_id();
    let mut c = MidiClip::new(id, "c", 0.0, 8.0);
    for &(pitch, start, length) in notes {
        c.notes.push(MidiNote {
            pitch,
            start,
            length,
            velocity: 127,
            channel: 0,
            pan: 0.0,
        });
    }
    for &(at, controller, value) in controls {
        c.controls.push(ControlEvent {
            at,
            controller,
            value,
            channel: 0,
        });
    }
    let tr = s.track_mut(t).unwrap();
    tr.midi.push(c);
    tr.instrument_plugin = Some(slot(cid));
    s
}

/// Adds an audio track with one click at `at` seconds.
fn click_track(s: &mut Session, at: f64) -> std::collections::HashMap<af2_core::MediaId, std::sync::Arc<AudioBuffer>> {
    let id = s.add_track_of(TrackKind::Audio);
    let m = s.alloc_id();
    let mut ch = vec![0.0f32; RATE as usize * 2];
    ch[(at * RATE as f64) as usize] = 0.25;
    s.media.push(MediaItem {
        id: m,
        name: "m".into(),
        path: None,
        duration: 2.0,
        rate: RATE,
        channels: 1,
        ext_id: None,
    });
    let cid = s.alloc_id();
    s.track_mut(id).unwrap().clips.push(Clip {
        props: Default::default(),
        id: cid,
        media: m,
        name: "c".into(),
        start: 0.0,
        offset: 0.0,
        length: 2.0,
        gain_db: 0.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
        ext_id: None,
    });
    let mut bufs = std::collections::HashMap::new();
    bufs.insert(
        m,
        std::sync::Arc::new(AudioBuffer {
            rate: RATE,
            channels: vec![ch],
        }),
    );
    bufs
}

fn export(s: &Session, bufs: &std::collections::HashMap<af2_core::MediaId, std::sync::Arc<AudioBuffer>>, secs: f64) -> Vec<f32> {
    let bufs = bufs.clone();
    let p = af2_engine::mix_program(s, &move |m| bufs.get(&m).cloned(), RATE);
    af2_engine::render(p, RATE, 0.0, secs).channels[0].clone()
}

#[test]
fn an_export_plays_a_midi_track_through_the_instrument_in_time() {
    let _g = serial();
    let c = setup();
    // A4 from beat 1 for a beat (0.5 s to 1 s at 120 BPM), and a click on
    // an audio track at 0.25 s: both land on time (the instrument's 16
    // samples are compensated, the click held back to match).
    let mut s = session(&c.cid, &[(69, 1.0, 1.0)], &[]);
    let bufs = click_track(&mut s, 0.25);
    let t = Instant::now();
    let out = export(&s, &bufs, 1.5);
    assert!(t.elapsed() > Duration::from_millis(500), "warmed up first");
    let click = 12_000;
    assert!((out[click] - 0.25).abs() < 1e-6, "click on time: {}", out[click]);
    assert!(peak(&out[..click]) < 1e-6 && peak(&out[click + 1..23_990]) < 1e-6);
    let at = first_sound(&out[click + 1..]).unwrap() + click + 1;
    assert!((24_000..=24_002).contains(&at), "note on time: {at}");
    let after = &out[48_100..];
    assert_eq!(peak(after), 0.0, "ends at 1 s");
    assert!(peak(&out[30_000..47_000]) > 0.5, "sounding");
}

#[test]
fn an_exports_pedal_holds_notes_to_where_it_comes_up() {
    let _g = serial();
    let c = setup();
    // The note ends at beat 1 (0.5 s); the pedal is down from beat 0.5
    // to beat 3 (1.5 s).
    let s = session(&c.cid, &[(60, 0.0, 1.0)], &[(0.5, 64, 1.0), (3.0, 64, 0.0)]);
    let out = export(&s, &Default::default(), 2.0);
    assert!(peak(&out[30_000..70_000]) > 0.5, "held");
    assert_eq!(peak(&out[72_100..]), 0.0, "let go at 1.5 s");
}

#[test]
fn a_crashing_instrument_leaves_the_rest_of_the_mix_playing() {
    let _g = serial();
    let c = setup();
    // Key 0 at beat 1 crashes the synth's process; the click track plays
    // on, on time, and the export finishes.
    let mut s = session(&c.cid, &[(69, 0.0, 1.0), (0, 1.0, 1.0)], &[]);
    let bufs = click_track(&mut s, 1.25);
    let out = export(&s, &bufs, 1.5);
    assert!(peak(&out[1000..20_000]) > 0.5, "played before the crash");
    let click = 60_000;
    assert!((out[click] - 0.25).abs() < 1e-6, "{}", out[click]);
    assert!(peak(&out[50_000..click]) < 1e-6, "silent after the crash");
    // A plug-in that is not installed: the track is silent, the rest plays.
    let mut s = session("00112233445566778899AABBCCDDEEFF", &[(69, 0.0, 2.0)], &[]);
    let bufs = click_track(&mut s, 0.5);
    let out = export(&s, &bufs, 1.0);
    assert!((out[24_000] - 0.25).abs() < 1e-6);
    assert!(peak(&out[..23_999]) < 1e-6);
}

#[test]
fn the_keys_play_a_copy_of_their_own_with_a_short_buffer() {
    let _g = serial();
    let c = setup();
    manager().set_keys_buffer(Some(128));
    let s = slot(&c.cid);
    let inst = build_instrument(&s, RATE, Use::Keys).unwrap();
    let key = format!("{}{}", s.plugin_data().inst, af2_vst3::live::KEYS);
    let live = manager().get(&key).expect("a copy of its own");
    assert_eq!(live.pipeline, 256);
    assert!(manager().get(&s.plugin_data().inst).is_none(), "playback's is apart");
    let t = Instant::now();
    while !live.running() && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(live.running(), "{:?}", live.status());
    let chain = af2_dsp::live::build_chain(&[], RATE, af2_engine::BLOCK);
    let mut keys = af2_engine::plugin_inst::KeysPlayer::new(inst, chain, [1.0, 1.0]);
    // Play in real time, 128 frames a callback: a key struck is heard
    // within the buffer and the plug-in's own delay.
    let n = 128;
    let pace = Duration::from_secs_f64(n as f64 / RATE as f64);
    let mut heard_at = None;
    for k in 0..400 {
        if k == 100 {
            keys.queue(NoteEvent::On {
                pitch: 69,
                velocity: 1.0,
                channel: 0,
                tuning: 0.0,
            });
        }
        let mut out = vec![0.0f32; n * 2];
        keys.render(&mut out, 2);
        if k >= 100 && heard_at.is_none() && peak(&out) > 0.1 {
            heard_at = Some(k);
        }
        std::thread::sleep(pace);
    }
    let k = heard_at.expect("heard");
    assert!(k - 100 <= (256 + 16) / n + 2, "heard {} callbacks late", k - 100);
    keys.queue(NoteEvent::Off {
        pitch: 69,
        channel: 0,
    });
    let mut last = 1.0;
    for _ in 0..40 {
        let mut out = vec![0.0f32; n * 2];
        keys.render(&mut out, 2);
        last = peak(&out);
        std::thread::sleep(pace);
    }
    assert_eq!(last, 0.0, "let go");
}

#[test]
fn a_lane_on_an_instrument_control_moves_it_as_the_export_plays() {
    use af2_core::automation::{Curve, Lane, LanePoint, LaneTarget};
    let _g = serial();
    let c = setup();
    // One note for 2 s; the synth's Level (control 0, gain = 2 x value)
    // held at 0.5 (gain 1) and then 0.125 (gain 0.25) from 1 s.
    let mut s = session(&c.cid, &[(69, 0.0, 4.0)], &[]);
    let tr = &mut s.tracks[0];
    tr.instrument_plugin.as_mut().unwrap().ext_id = Some("inst".into());
    let mut lane = Lane::new(LaneTarget::Effect {
        slot: "inst".into(),
        param: 0,
        key: Some("p0".into()),
    });
    lane.points = vec![
        LanePoint {
            time: 0.0,
            value: 0.5,
            curve: Curve::Hold,
        },
        LanePoint {
            time: 1.0,
            value: 0.125,
            curve: Curve::Linear,
        },
    ];
    tr.lanes.push(lane);
    let out = export(&s, &Default::default(), 2.0);
    let (loud, soft) = (peak(&out[10_000..45_000]), peak(&out[55_000..90_000]));
    assert!(loud > 0.5, "{loud}");
    assert!((soft / loud - 0.25).abs() < 0.02, "{loud} then {soft}");
}
