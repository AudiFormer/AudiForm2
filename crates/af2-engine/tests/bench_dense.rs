// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The same dense session Audiform's own benchmark renders (48 stereo
//! tracks, clips every 8 s that crossfade, EQ + compressor on every track,
//! a reverb on every third, sends to two reverb buses, every seventh track
//! routed to a bus), so the two engines can be compared like for like:
//!
//! ```text
//! cargo test --release -p af2-engine --test bench_dense -- --ignored --nocapture
//! ```
//!
//! `BENCH_TRACKS` (48) and `BENCH_SECS` (30) change the session.

use std::sync::Arc;
use std::time::Instant;

use af2_core::{AudioBuffer, Clip, EffectSlot, MediaItem, Send, Session, TrackKind};
use af2_engine::{mix_program, render};

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn slot(id: &str, set: &[(usize, f32)]) -> EffectSlot {
    let e = af2_dsp::Effect::from_id(id).unwrap();
    let mut values = e.defaults();
    for (i, v) in set {
        values[*i] = *v;
    }
    EffectSlot {
        position: None,
        sidechain: None,
        effect: id.into(),
        values,
        bypass: false,
        data: None,
        ext_id: None,
    }
}

/// A minute of stereo music-like sound: two tones and a little noise
/// (the same recipe as Audiform's benchmark file).
fn music(rate: u32) -> AudioBuffer {
    let n = rate as usize * 60;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    let mut seed = 1u32;
    for i in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        let t = i as f32 / rate as f32;
        let v = 0.2 * (t * 220.0 * std::f32::consts::TAU).sin()
            + 0.1 * (t * 331.0 * std::f32::consts::TAU).sin()
            + 0.05 * noise;
        l.push(v);
        r.push(v * 0.9);
    }
    AudioBuffer {
        rate,
        channels: vec![l, r],
    }
}

/// The dense session: `tracks` tracks over `secs` seconds.
fn dense(tracks: usize, secs: f64) -> (Session, Arc<AudioBuffer>, usize) {
    let rate = 48_000u32;
    let buf = Arc::new(music(rate));
    let mut s = Session::new("bench", rate, 0);
    let m = s.alloc_id();
    s.media.push(MediaItem {
        id: m,
        name: "bench".into(),
        path: None,
        duration: 60.0,
        rate,
        channels: 2,
        ext_id: None,
    });
    let buses: Vec<_> = (0..2)
        .map(|_| {
            let b = s.add_track_of(TrackKind::Bus);
            let i = s.track_index(b).unwrap();
            // Fully wet.
            s.tracks[i].chain.push(slot("reverb", &[(2, 1.0)]));
            b
        })
        .collect();
    let mut clips = 0;
    for t in 0..tracks {
        let id = s.add_track();
        let i = s.track_index(id).unwrap();
        let tr = &mut s.tracks[i];
        tr.volume_db = -6.0;
        tr.pan = (t % 5) as f32 * 0.4 - 0.8;
        tr.chain.push(slot("parametric-eq", &[(1, 2.0), (6, -1.5)]));
        tr.chain.push(slot("compressor", &[(0, -18.0), (1, 3.0)]));
        if t % 3 == 0 {
            tr.chain.push(slot("reverb", &[(2, 0.2)]));
        }
        tr.sends.push(Send {
            bus: buses[t % 2],
            level_db: -12.0,
            pre_fader: false,
            pan: 0.0,
            enabled: true,
            ext_id: None,
        });
        if t % 7 == 6 {
            tr.output = Some(buses[t % 2]);
        }
        let mut at = (t % 4) as f64 * 0.25;
        let mut k = 0u64;
        while at < secs {
            let src = ((t as u64 * 7_919) % (48_000 * 40)) as f64 / rate as f64;
            tr.clips.push(Clip {
                props: Default::default(),
                id: 10_000 + t as u64 * 1000 + k,
                media: m,
                name: "c".into(),
                start: at,
                offset: src,
                length: 10.0,
                gain_db: -1.0,
                fade_in: 1.0,
                fade_out: 2.0,
                muted: false,
                ext_id: None,
            });
            clips += 1;
            k += 1;
            at += 8.0;
        }
    }
    (s, buf, clips)
}

#[test]
#[ignore]
fn bench_dense() {
    let rate = 48_000u32;
    let tracks = env("BENCH_TRACKS", 48);
    let secs = env("BENCH_SECS", 30) as f64;
    let (s, buf, clips) = dense(tracks, secs);
    let b2 = buf.clone();
    let program = mix_program(&s, &move |_| Some(b2.clone()), rate);
    let t0 = Instant::now();
    let out = render(program, rate, 0.0, secs);
    let took = t0.elapsed().as_secs_f64();
    eprintln!(
        "{tracks} tracks, {clips} clips, fx true: {secs} s rendered in {took:.2} s ({:.1}x real time)",
        secs / took
    );
    assert_eq!(out.frames(), (secs * rate as f64).round() as usize);
}

/// Seconds of stereo audio each effect gets through per second of work.
#[test]
#[ignore]
fn bench_each_effect() {
    let rate = 48_000u32;
    let buf = music(rate);
    let n = rate as usize * 30;
    for (id, set) in [
        ("parametric-eq", vec![(1usize, 2.0f32), (6, -1.5)]),
        ("compressor", vec![(0, -18.0), (1, 3.0)]),
        ("reverb", vec![(2, 0.2)]),
        ("limiter", vec![]),
    ] {
        let s = slot(id, &set);
        let mut p = af2_dsp::live::build_slot(&s, rate).unwrap();
        let mut l = buf.channels[0][..n].to_vec();
        let mut r = buf.channels[1][..n].to_vec();
        let t0 = Instant::now();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            p.process(a, b);
        }
        let took = t0.elapsed().as_secs_f64();
        eprintln!(
            "{id}: {:.0}x real time ({:.1} ms per 30 s)",
            30.0 / took,
            took * 1000.0
        );
    }
}

/// Spreading tracks over threads changes nothing in the sound.
#[test]
fn parallel_mix_matches_serial_exactly() {
    let rate = 48_000u32;
    let (s, buf, _) = dense(12, 3.0);
    let run = |parallel: bool| {
        let b = buf.clone();
        let mut m = af2_engine::mix::build(&s, &move |_| Some(b.clone()), rate);
        m.parallel = parallel;
        render(af2_engine::Program::Mix(Box::new(m)), rate, 0.0, 3.0)
    };
    let a = run(true);
    let b = run(false);
    assert_eq!(a.channels, b.channels);
    assert!(a.channels[0].iter().any(|v| v.abs() > 1e-3));
}
