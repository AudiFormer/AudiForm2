// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Tests for sidechain keys, rack levels and the metronome options.

use std::collections::HashMap;
use std::sync::Arc;

use af27_core::midi::{ClickSound, MetronomeSettings, Subdivide};
use af27_core::session::{Clip, MediaItem};
use af27_core::{AudioBuffer, EffectSlot, MediaId, Rack, Session, TrackId, TrackKind};

use crate::synth::{click_sound, click_with};
use crate::{mix, mix_program, render, Cmd, Garbage, Plan, Program, Region, Voice, MASTER};

const RATE: u32 = 8000;

/// A session at `RATE` and the audio of its media.
struct Fixture {
    s: Session,
    bufs: HashMap<MediaId, Arc<AudioBuffer>>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            s: Session::new("keys", RATE, 0),
            bufs: HashMap::new(),
        }
    }

    /// A track of `kind` holding a steady `level` from `start` for `len` s.
    fn track(&mut self, kind: TrackKind, level: f32, start: f64, len: f64) -> TrackId {
        let id = self.s.add_track_of(kind);
        if kind == TrackKind::Bus || level == 0.0 {
            return id;
        }
        let m = self.s.alloc_id();
        let frames = (len * RATE as f64) as usize;
        self.s.media.push(MediaItem {
            id: m,
            name: "m".into(),
            path: None,
            duration: len,
            rate: RATE,
            channels: 1,
            ext_id: None,
        });
        self.bufs.insert(
            m,
            Arc::new(AudioBuffer {
                rate: RATE,
                channels: vec![vec![level; frames]],
            }),
        );
        let cid = self.s.alloc_id();
        self.s.track_mut(id).unwrap().clips.push(Clip {
            props: Default::default(),
            id: cid,
            media: m,
            name: "c".into(),
            start,
            offset: 0.0,
            length: len,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            ext_id: None,
        });
        id
    }

    fn t(&mut self, id: TrackId) -> &mut af27_core::Track {
        self.s.track_mut(id).unwrap()
    }

    fn program(&self) -> Program {
        let bufs = self.bufs.clone();
        mix_program(&self.s, &move |m| bufs.get(&m).cloned(), RATE)
    }

    fn mix(&self) -> mix::Mix {
        let bufs = self.bufs.clone();
        mix::build(&self.s, &move |m| bufs.get(&m).cloned(), RATE)
    }

    /// The left channel of a render from 0 to `to`.
    fn render(&self, to: f64) -> Vec<f32> {
        render(self.program(), RATE, 0.0, to).channels[0].clone()
    }
}

/// A compressor that only works on sound above -10 dBFS, hard (10:1),
/// quick to catch and to let go, with no knee or makeup.
fn compressor(key: Option<TrackId>) -> EffectSlot {
    EffectSlot {
        sidechain: key,
        effect: "compressor".into(),
        values: vec![-10.0, 10.0, 1.0, 20.0, 0.0, 0.0],
        bypass: false,
        data: None,
        ext_id: None,
    }
}

fn amp(db: f32) -> EffectSlot {
    EffectSlot {
        sidechain: None,
        effect: "amplify".into(),
        values: vec![db],
        bypass: false,
        data: None,
        ext_id: None,
    }
}

/// The level of `x` at `secs`.
fn at(x: &[f32], secs: f64) -> f32 {
    x[(secs * RATE as f64) as usize]
}

/// Music (steady 0.25, -12 dBFS) for 2 s, and a voice (0.9) from 1 s,
/// its fader all the way down so only its key is heard.
fn music_and_voice() -> (Fixture, TrackId, TrackId) {
    let mut f = Fixture::new();
    let music = f.track(TrackKind::Audio, 0.25, 0.0, 2.0);
    let voice = f.track(TrackKind::Audio, 0.9, 1.0, 1.0);
    f.t(voice).volume_db = -200.0;
    (f, music, voice)
}

#[test]
fn a_keyed_compressor_ducks_only_while_the_key_is_loud() {
    let (mut f, music, voice) = music_and_voice();
    f.t(music).chain.push(compressor(Some(voice)));
    let out = f.render(2.0);
    // Before the voice: the music is under the threshold, untouched.
    assert!((at(&out, 0.5) - 0.25).abs() < 1e-3, "{}", at(&out, 0.5));
    // While the voice plays (pre-fader, though its fader is down): ducked.
    let ducked = at(&out, 1.5);
    assert!(ducked < 0.25 * 0.5, "{ducked}");
    assert!(ducked > 0.0);
}

#[test]
fn an_unkeyed_or_unresolvable_key_hears_its_own_input_never_silence() {
    let (mut f, music, voice) = music_and_voice();
    // Loud music (0.9) over the threshold, so its own level compresses it.
    f.t(music).clips[0].gain_db = 11.0846; // 0.25 -> 0.9
    let mut own = Vec::new();
    for key in [None, Some(9999), Some(music)] {
        f.t(music).chain = vec![compressor(key)];
        own.push(f.render(0.9));
    }
    let x = at(&own[0], 0.5);
    assert!(x < 0.8, "compressed by its own level: {x}");
    // A key naming no track, or the track itself: its own input.
    assert_eq!(own[0], own[1]);
    assert_eq!(own[0], own[2]);
    // Keys are taken before the fader and mute: a muted voice still
    // ducks the music.
    f.t(music).clips[0].gain_db = 0.0;
    f.t(music).chain = vec![compressor(Some(voice))];
    f.t(voice).mute = true;
    let muted = f.render(2.0);
    assert!(at(&muted, 1.5) < 0.25 * 0.5, "{}", at(&muted, 1.5));
    assert!((at(&muted, 0.5) - 0.25).abs() < 1e-3);
    f.t(voice).mute = false;
    // An effect that takes no key ignores one.
    f.t(voice).mute = false;
    f.t(music).chain = vec![amp(0.0)];
    let plain = f.render(2.0);
    f.t(music).chain[0].sidechain = Some(voice);
    assert_eq!(f.render(2.0), plain);
    let m = f.mix();
    assert_eq!(m.tracks[0].keys, vec![None]);
    assert!(!m.tracks[1].is_key());
}

#[test]
fn a_track_keyed_by_a_bus_runs_after_the_bus_and_what_feeds_it() {
    // In session order: the music, then the bus, then the voice feeding it.
    let mut f = Fixture::new();
    let music = f.track(TrackKind::Audio, 0.25, 0.0, 2.0);
    let bus = f.track(TrackKind::Bus, 0.0, 0.0, 0.0);
    let voice = f.track(TrackKind::Audio, 0.9, 1.0, 1.0);
    f.t(voice).output = Some(bus);
    f.t(bus).volume_db = -200.0;
    f.t(music).chain.push(compressor(Some(bus)));
    let m = f.mix();
    let level = |i: usize| m.levels().iter().position(|l| l.contains(&i)).unwrap();
    assert!(
        level(2) < level(1) && level(1) < level(0),
        "{:?}",
        m.levels()
    );
    assert_eq!(m.tracks[0].keys, vec![Some(1)]);
    let out = f.render(2.0);
    assert!((at(&out, 0.5) - 0.25).abs() < 1e-3);
    assert!(at(&out, 1.5) < 0.25 * 0.5, "{}", at(&out, 1.5));
    // The same mix in one piece or spread over the engine's threads.
    let mut serial = f.mix();
    serial.parallel = false;
    let mut par = f.mix();
    let mut a = vec![0.0; mix::BLOCK * 2];
    let mut b = vec![0.0; mix::BLOCK * 2];
    let dt = 1.0 / RATE as f64;
    for k in 0..8 {
        let t0 = k as f64 * mix::BLOCK as f64 * dt;
        serial.render(t0, dt, mix::BLOCK, &mut a, false);
        par.render(t0, dt, mix::BLOCK, &mut b, false);
        assert_eq!(a, b);
    }
}

#[test]
fn key_loops_are_dropped_and_never_hang() {
    let mut f = Fixture::new();
    let a = f.track(TrackKind::Audio, 0.5, 0.0, 1.0);
    let b = f.track(TrackKind::Audio, 0.5, 0.0, 1.0);
    let bus = f.track(TrackKind::Bus, 0.0, 0.0, 0.0);
    let c = f.track(TrackKind::Audio, 0.5, 0.0, 1.0);
    // a and b key each other; c feeds the bus and is keyed by it; the bus
    // keys itself.
    f.t(a).chain.push(compressor(Some(b)));
    f.t(b).chain.push(compressor(Some(a)));
    f.t(c).output = Some(bus);
    f.t(c).chain.push(compressor(Some(bus)));
    f.t(bus).chain.push(compressor(Some(bus)));
    let m = f.mix();
    // The first key of the a-b loop stays; the one closing it goes.
    assert_eq!(m.tracks[0].keys, vec![Some(1)]);
    assert_eq!(m.tracks[1].keys, vec![None]);
    assert_eq!(m.tracks[2].keys, vec![None]);
    assert_eq!(m.tracks[3].keys, vec![None]);
    let all: usize = m.levels().iter().map(Vec::len).sum();
    assert_eq!(all, 4, "every track runs once: {:?}", m.levels());
    let out = f.render(1.0);
    assert!(out.iter().all(|v| v.is_finite()));
    assert!(out.iter().any(|v| v.abs() > 0.1));
}

#[test]
fn without_keys_the_order_is_tracks_then_buses() {
    let mut f = Fixture::new();
    let a = f.track(TrackKind::Audio, 0.5, 0.0, 1.0);
    let b1 = f.track(TrackKind::Bus, 0.0, 0.0, 0.0);
    let b2 = f.track(TrackKind::Bus, 0.0, 0.0, 0.0);
    let _c = f.track(TrackKind::Midi, 0.0, 0.0, 0.0);
    f.t(a).output = Some(b1);
    f.t(b1).output = Some(b2);
    let m = f.mix();
    assert_eq!(m.levels(), &[vec![0, 3], vec![1], vec![2]]);
}

#[test]
fn the_rack_scales_mixes_and_bypasses_the_chain() {
    let mut f = Fixture::new();
    let t = f.track(TrackKind::Audio, 0.5, 0.0, 1.0);
    f.t(t).chain.push(amp(6.0206)); // x2
    let run = |f: &Fixture| at(&f.render(0.5), 0.25);
    assert!((run(&f) - 1.0).abs() < 1e-3);
    // Input -6 dB, then the x2 effect, half wet, then output +6 dB:
    // (0.5 * 0.5) dry 0.25, wet 0.5, mixed 0.375, out 0.75.
    f.t(t).rack = Rack {
        bypass: false,
        input_db: -6.0206,
        output_db: 6.0206,
        mix: 0.5,
    };
    assert!((run(&f) - 0.75).abs() < 1e-3, "{}", run(&f));
    // Every slot bypassed: the gains still apply.
    f.t(t).chain[0].bypass = true;
    assert!((run(&f) - 0.5).abs() < 1e-3, "{}", run(&f));
    // The rack off: untouched.
    f.t(t).chain[0].bypass = false;
    f.t(t).rack.bypass = true;
    assert!((run(&f) - 0.5).abs() < 1e-3, "{}", run(&f));
    // The master rack, with no effects on the master.
    f.t(t).rack = Rack::default();
    f.s.master_rack.output_db = -6.0206;
    assert!((run(&f) - 0.5).abs() < 1e-3, "{}", run(&f));
}

#[test]
fn rack_changes_apply_live() {
    let mut f = Fixture::new();
    let t = f.track(TrackKind::Audio, 0.5, 0.0, 10.0);
    f.t(t).chain.push(amp(0.0));
    let (mut trash, _rx) = rtrb::RingBuffer::<Garbage>::new(4);
    let mut v = Voice {
        program: Some(Box::new(f.program())),
        audition: None,
        playing: false,
        t: 0.0,
        plan: Plan::default(),
        metro: Default::default(),
        recording: false,
        clicks: None,
    };
    v.apply(
        Cmd::Play {
            from: 0.0,
            plan: Plan::new(10.0, Region::None),
        },
        &mut trash,
    );
    let mut out = vec![0.0f32; 64];
    let mut level = |v: &mut Voice| {
        v.render(&mut out, 2, RATE as f64, false);
        out[0]
    };
    assert!((level(&mut v) - 0.5).abs() < 1e-4);
    let rack = Rack {
        input_db: -6.0206,
        ..Rack::default()
    };
    v.apply(Cmd::Rack { track: 0, rack }, &mut trash);
    assert!((level(&mut v) - 0.25).abs() < 1e-3);
    let rack = Rack {
        output_db: -6.0206,
        ..Rack::default()
    };
    v.apply(
        Cmd::Rack {
            track: MASTER,
            rack,
        },
        &mut trash,
    );
    assert!((level(&mut v) - 0.125).abs() < 1e-3);
    v.apply(
        Cmd::Rack {
            track: 0,
            rack: Rack::default(),
        },
        &mut trash,
    );
    assert!((level(&mut v) - 0.25).abs() < 1e-3);
}

// The metronome.

fn segs(bpm: f64) -> Vec<af27_core::midi::Segment> {
    let t = af27_core::midi::Tempo {
        bpm,
        ..Default::default()
    };
    t.segments().collect()
}

/// The loudest click in the 80 ms from `t`.
fn peak(t: f64, segs: &[af27_core::midi::Segment], m: &MetronomeSettings) -> f32 {
    (0..1600)
        .map(|i| click_with(t + i as f64 / 20_000.0, segs, m).abs())
        .fold(0.0, f32::max)
}

#[test]
fn clicks_land_on_beats_with_the_downbeat_accented() {
    let s = segs(120.0); // a beat every 0.5 s, four to the bar
    let m = MetronomeSettings {
        level_db: 0.0,
        ..Default::default()
    };
    // Downbeat: 1.5x pitch, level 1.0; other beats 0.75; nothing between.
    let local = 0.0004f32;
    let want = click_sound(ClickSound::Tick, local, 1.5);
    assert!((click_with(local as f64, &s, &m) - want).abs() < 1e-5);
    let want = click_sound(ClickSound::Tick, local, 1.0) * 0.75;
    assert!((click_with(0.5 + local as f64, &s, &m) - want).abs() < 1e-5);
    assert!(peak(0.0, &s, &m) > 0.9);
    assert!((peak(0.5, &s, &m) - 0.75 * peak(0.0, &s, &m)).abs() < 0.05);
    assert!(peak(2.0, &s, &m) > 0.9, "the next bar");
    assert_eq!(click_with(0.25, &s, &m), 0.0);
    assert_eq!(click_with(0.3, &s, &m), 0.0);
    // No accent: the downbeat is a beat like the others.
    let flat = MetronomeSettings { accent: false, ..m };
    let want = click_sound(ClickSound::Tick, local, 1.0) * 0.75;
    assert!((click_with(local as f64, &s, &flat) - want).abs() < 1e-5);
    // Level and mute.
    let quiet = MetronomeSettings {
        level_db: -20.0,
        ..m
    };
    assert!((peak(0.0, &s, &quiet) - 0.1 * peak(0.0, &s, &m)).abs() < 0.01);
    let muted = MetronomeSettings { muted: true, ..m };
    assert_eq!(peak(0.0, &s, &muted), 0.0);
}

#[test]
fn subdivisions_click_softly_between_the_beats() {
    let s = segs(120.0);
    for (sub, per) in [
        (Subdivide::Halves, 2),
        (Subdivide::Triplets, 3),
        (Subdivide::Quarters, 4),
    ] {
        let m = MetronomeSettings {
            level_db: 0.0,
            subdivide: sub,
            sound: ClickSound::Beep,
            ..Default::default()
        };
        let step = 0.5 / per as f64;
        let beat = peak(0.5, &s, &m);
        for k in 1..per {
            let p = peak(0.5 + step * k as f64, &s, &m);
            assert!((p - beat * 0.4 / 0.75).abs() < 0.03, "{sub:?} {k}: {p}");
        }
        // Silence just before each subdivision.
        let gap = click_with(0.5 + step - 0.002, &s, &m);
        assert!(step < 0.08 || gap == 0.0, "{sub:?}");
    }
}

#[test]
fn every_click_sound_is_short_bounded_and_repeatable() {
    for sound in [
        ClickSound::Tick,
        ClickSound::Wood,
        ClickSound::Beep,
        ClickSound::Shaker,
    ] {
        let xs: Vec<f32> = (0..1000)
            .map(|i| click_sound(sound, i as f32 / 10_000.0, 1.0))
            .collect();
        assert!(
            xs.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
            "{sound:?}"
        );
        assert!(xs.iter().any(|v| v.abs() > 0.3), "{sound:?} sounds");
        assert_eq!(click_sound(sound, 0.081, 1.0), 0.0, "{sound:?} ends");
        let again: Vec<f32> = (0..1000)
            .map(|i| click_sound(sound, i as f32 / 10_000.0, 1.0))
            .collect();
        assert_eq!(xs, again);
    }
    // The beep holds, then fades out by 60 ms.
    assert!(click_sound(ClickSound::Beep, 0.0405, 1.0).abs() > 0.5);
    assert!(click_sound(ClickSound::Beep, 0.0601, 1.0).abs() < 1e-6);
}

#[test]
fn the_click_follows_tempo_and_time_signature_changes() {
    use af27_core::midi::{Tempo, TempoChange};
    // One bar of 4/4 at 120 (2 s), then 3/4 at 60: a beat a second.
    let mut t = Tempo::default();
    t.changes.push(TempoChange::new(1, 60.0, 3, 4));
    let s: Vec<_> = t.segments().collect();
    let m = MetronomeSettings::default();
    assert!(peak(2.0, &s, &m) > peak(3.0, &s, &m) * 1.2, "a new bar");
    assert!(peak(3.0, &s, &m) > 0.1);
    assert_eq!(click_with(2.5, &s, &m), 0.0);
    assert!(
        peak(5.0, &s, &m) > peak(4.0, &s, &m) * 1.2,
        "three beats on"
    );
}

fn metronome_session() -> Fixture {
    let mut f = Fixture::new();
    f.track(TrackKind::Audio, 0.0, 0.0, 0.0);
    f.s.tempo.metronome = true;
    f
}

#[test]
fn the_click_never_reaches_a_render() {
    let f = metronome_session();
    let out = render(f.program(), RATE, 0.0, 2.0);
    assert!(out.channels.iter().flatten().all(|v| *v == 0.0));
}

/// A live voice playing `region` from `from`.
fn live_voice(f: &Fixture, from: f64, plan: Plan) -> (Voice, rtrb::Producer<Garbage>) {
    let (mut trash, rx) = rtrb::RingBuffer::<Garbage>::new(4);
    std::mem::forget(rx);
    let mut v = Voice {
        program: Some(Box::new(f.program())),
        audition: None,
        playing: false,
        t: 0.0,
        plan: Plan::default(),
        metro: MetronomeSettings::default(),
        recording: false,
        clicks: Some(vec![0.0; crate::CLICK_FRAMES]),
    };
    v.apply(Cmd::Play { from, plan }, &mut trash);
    (v, trash)
}

#[test]
fn live_the_click_is_kept_apart_from_the_meters_and_follows_loops() {
    let f = metronome_session();
    // Loop 0.5..0.7 s: the beat at 0.5 comes round again every 0.2 s.
    let plan = Plan::new(
        f64::INFINITY,
        Region::Loop {
            start: 0.5,
            end: 0.7,
        },
    );
    let (mut v, _trash) = live_voice(&f, 0.5, plan);
    let frames = (0.5 * RATE as f64) as usize;
    let mut out = vec![0.0f32; frames * 2];
    let peaks = v.render(&mut out, 2, RATE as f64, false);
    // The mix (and so the meters and scope) has no click in it.
    assert_eq!(peaks, (0.0, 0.0));
    assert!(out.iter().all(|x| *x == 0.0));
    let c = v.clicks.as_ref().unwrap();
    let loud = |a: usize| c[a..a + 40].iter().any(|x| x.abs() > 0.05);
    let lap = (0.2 * RATE as f64) as usize;
    assert!(loud(0) && loud(lap) && loud(2 * lap), "at each loop start");
    assert!(!loud(lap / 2), "nothing between");
    // Heard last: added to what goes out.
    crate::add_clicks(&mut out, 2, c);
    assert!(out.iter().any(|x| x.abs() > 0.05));
    assert!(out.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]));
}

/// Moves a playing voice to `t` without a reset.
fn seek(v: &mut Voice, trash: &mut rtrb::Producer<Garbage>, t: f64) {
    let plan = Plan::new(f64::INFINITY, Region::None);
    v.apply(Cmd::Retarget { to: Some(t), plan }, trash);
}

#[test]
fn live_click_only_while_recording_and_off_switches() {
    let f = metronome_session();
    let (mut v, mut trash) = live_voice(&f, 0.0, Plan::new(f64::INFINITY, Region::None));
    let mut out = vec![0.0f32; 800 * 2];
    let mut heard = |v: &mut Voice| {
        v.render(&mut out, 2, RATE as f64, false);
        v.clicks.as_ref().unwrap()[..800]
            .iter()
            .any(|x| x.abs() > 0.01)
    };
    let only_rec = MetronomeSettings {
        recording_only: true,
        ..Default::default()
    };
    v.apply(Cmd::MetronomeSettings(only_rec), &mut trash);
    seek(&mut v, &mut trash, 0.0);
    assert!(!heard(&mut v), "not recording");
    v.apply(Cmd::Recording(true), &mut trash);
    seek(&mut v, &mut trash, 0.5);
    assert!(heard(&mut v), "recording");
    v.apply(Cmd::Metronome(false), &mut trash);
    seek(&mut v, &mut trash, 1.0);
    assert!(!heard(&mut v), "switched off");
    v.apply(Cmd::Metronome(true), &mut trash);
    let muted = MetronomeSettings {
        muted: true,
        ..only_rec
    };
    v.apply(Cmd::MetronomeSettings(muted), &mut trash);
    seek(&mut v, &mut trash, 1.5);
    assert!(!heard(&mut v), "muted");
}
