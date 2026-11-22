// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Tests for input monitoring through the mix: a monitored track's input
//! goes through its chain, fader and pan, stopped and playing, and only
//! that track's.

use std::sync::Arc;

use af2_core::session::{Clip, MediaItem};
use af2_core::{AudioBuffer, EffectSlot, Session, TrackId, TrackKind};

use crate::monitor::{self, Feed, FeedLane};
use crate::transport::{Plan, Region};
use crate::{mix, Program, Voice, BLOCK};

const RATE: u32 = 8000;

fn amp(db: f32) -> EffectSlot {
    EffectSlot {
        position: None,
        sidechain: None,
        effect: "amplify".into(),
        values: vec![db],
        bypass: false,
        data: None,
        ext_id: None,
    }
}

/// A session with one audio track playing a steady `clip` level for a
/// few seconds (0: no clip), its chain `+db`.
fn session(clip: f32, db: f32) -> (Session, TrackId, Option<Arc<AudioBuffer>>) {
    let mut s = Session::new("mon", RATE, 0);
    let id = s.add_track_of(TrackKind::Audio);
    s.track_mut(id).unwrap().chain.push(amp(db));
    if clip == 0.0 {
        return (s, id, None);
    }
    let m = s.alloc_id();
    let n = RATE as usize * 4;
    s.media.push(MediaItem {
        id: m,
        name: "m".into(),
        path: None,
        duration: 4.0,
        rate: RATE,
        channels: 2,
        ext_id: None,
    });
    let buf = Arc::new(AudioBuffer {
        rate: RATE,
        channels: vec![vec![clip; n], vec![clip; n]],
    });
    let cid = s.alloc_id();
    s.track_mut(id).unwrap().clips.push(Clip {
        props: Default::default(),
        id: cid,
        media: m,
        name: "c".into(),
        start: 0.0,
        offset: 0.0,
        length: 4.0,
        gain_db: 0.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
        ext_id: None,
    });
    (s, id, Some(buf))
}

fn voice(s: &Session, buf: Option<Arc<AudioBuffer>>, playing: bool) -> Voice {
    let mut m = mix::build(s, &move |_| buf.clone(), RATE);
    // Clips read straight from memory, as an export does.
    m.set_offline();
    m.prepare_now();
    m.start(0.0);
    Voice {
        program: Some(Box::new(Program::Mix(Box::new(m)))),
        audition: None,
        playing,
        t: 0.0,
        plan: Plan::new(4.0, Region::None),
        metro: Default::default(),
        recording: false,
        clicks: None,
        declick: Default::default(),
    }
}

/// A feed of one mono device carrying `level`, heard on `track`.
fn feed(track: TrackId) -> (rtrb::Producer<f32>, Feed) {
    let (tx, rx) = monitor::ring(RATE, 1);
    let f = Feed::new(
        vec![(rx, 1)],
        vec![FeedLane {
            track,
            dev: 0,
            pick: [0, 0],
        }],
    );
    (tx, f)
}

/// Renders `blocks` callbacks of BLOCK frames, the input carrying `level`
/// throughout; the last frame's left and right.
fn run(v: &mut Voice, tx: &mut rtrb::Producer<f32>, f: &mut Feed, level: f32, blocks: usize) -> (f32, f32) {
    let mut out = vec![0.0f32; BLOCK * 2];
    for _ in 0..blocks {
        for _ in 0..BLOCK {
            tx.push(level).unwrap();
        }
        f.begin(BLOCK);
        f.silence();
        v.render_live(&mut out, 2, RATE as f64, false, Some(f));
    }
    (out[BLOCK * 2 - 2], out[BLOCK * 2 - 1])
}

/// What the track gives out for a clip of `level` played normally.
fn played(level: f32, db: f32) -> (f32, f32) {
    let (s, _, buf) = session(level, db);
    let b = crate::render(
        crate::mix_program(&s, &move |_| buf.clone(), RATE),
        RATE,
        0.0,
        (BLOCK * 4) as f64 / RATE as f64,
    );
    let i = BLOCK * 4 - 1;
    (b.channels[0][i], b.channels[1][i])
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn stopped_the_input_is_heard_through_the_chain_and_the_clips_are_not() {
    let want = played(0.1, 6.0);
    assert!(want.0 > 0.15, "the chain lifts it: {want:?}");
    // The track has a loud clip under the playhead: stopped, it is silent.
    let (s, id, buf) = session(0.5, 6.0);
    let mut v = voice(&s, buf, false);
    let (mut tx, mut f) = feed(id);
    let got = run(&mut v, &mut tx, &mut f, 0.1, 4);
    assert!(near(got.0, want.0) && near(got.1, want.1), "{got:?} vs {want:?}");
    // Still stopped where it was.
    assert_eq!(v.t, 0.0);
}

#[test]
fn playing_the_input_joins_the_clips_before_the_chain() {
    let clip = played(0.2, 6.0);
    let input = played(0.1, 6.0);
    let (s, id, buf) = session(0.2, 6.0);
    let mut v = voice(&s, buf, true);
    let (mut tx, mut f) = feed(id);
    assert!(clip.0 > 0.0 && input.0 > 0.0);
    let got = run(&mut v, &mut tx, &mut f, 0.1, 4);
    assert!(near(got.0, clip.0 + input.0), "{got:?}");
    assert!(v.t > 0.0);
}

#[test]
fn the_fader_pan_and_mute_apply() {
    let (mut s, id, _) = session(0.0, 0.0);
    s.track_mut(id).unwrap().pan = -1.0;
    let mut v = voice(&s, None, false);
    let (mut tx, mut f) = feed(id);
    let (l, r) = run(&mut v, &mut tx, &mut f, 0.1, 4);
    assert!(l > 0.05 && r.abs() < 1e-6, "hard left: {l} {r}");
    s.track_mut(id).unwrap().mute = true;
    let mut v = voice(&s, None, false);
    let (mut tx, mut f) = feed(id);
    let (l, r) = run(&mut v, &mut tx, &mut f, 0.1, 4);
    assert_eq!((l, r), (0.0, 0.0), "muted");
}

#[test]
fn only_the_monitored_track_hears_its_input() {
    let (mut s, id, _) = session(0.0, 0.0);
    let other = s.add_track_of(TrackKind::Audio);
    s.track_mut(id).unwrap().mute = true;
    let mut v = voice(&s, None, false);
    // The feed is for the muted track: the other one stays silent.
    let (mut tx, mut f) = feed(id);
    assert_eq!(run(&mut v, &mut tx, &mut f, 0.3, 3), (0.0, 0.0));
    let (mut tx, mut f) = feed(other);
    let (l, _) = run(&mut v, &mut tx, &mut f, 0.3, 3);
    assert!(l > 0.1);
}

#[test]
fn without_a_feed_nothing_changes() {
    let want = played(0.2, 3.0);
    let (s, _, buf) = session(0.2, 3.0);
    let mut v = voice(&s, buf, true);
    let mut out = vec![0.0f32; BLOCK * 2];
    for _ in 0..4 {
        v.render_live(&mut out, 2, RATE as f64, false, None);
    }
    assert_eq!((out[BLOCK * 2 - 2], out[BLOCK * 2 - 1]), want);
    // Stopped without a feed: silence, as before.
    let mut v = voice(&s, None, false);
    v.render_live(&mut out, 2, RATE as f64, false, None);
    assert!(out.iter().all(|&x| x == 0.0));
}
