// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Tests for the mix graph: the send law, solo, delay compensation,
//! ramps, mono and polarity, automation playback and damaged routing.

use std::collections::HashMap;
use std::sync::Arc;

use af2_core::automation::{Curve, Lane, LanePoint, LaneTarget};
use af2_core::session::{Clip, MediaItem};
use af2_core::{AudioBuffer, EffectSlot, MediaId, Send, Session, TrackId, TrackKind};

use crate::mix::{self, LaneKind, BLOCK};
use crate::{mix_program, render};

const RATE: u32 = 8000;

struct Fx {
    s: Session,
    bufs: HashMap<MediaId, Arc<AudioBuffer>>,
}

impl Fx {
    fn new() -> Fx {
        Fx {
            s: Session::new("mix", RATE, 0),
            bufs: HashMap::new(),
        }
    }

    /// A track playing `left` and `right` (one value per frame) from 0.
    fn track_lr(&mut self, left: Vec<f32>, right: Vec<f32>) -> TrackId {
        let id = self.s.add_track_of(TrackKind::Audio);
        let m = self.s.alloc_id();
        let len = left.len() as f64 / RATE as f64;
        self.s.media.push(MediaItem {
            id: m,
            name: "m".into(),
            path: None,
            duration: len,
            rate: RATE,
            channels: 2,
            ext_id: None,
        });
        self.bufs.insert(
            m,
            Arc::new(AudioBuffer {
                rate: RATE,
                channels: vec![left, right],
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
            length: len,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
            ext_id: None,
        });
        id
    }

    /// A track of a steady `level` for `secs`.
    fn steady(&mut self, level: f32, secs: f64) -> TrackId {
        let n = (secs * RATE as f64) as usize;
        self.track_lr(vec![level; n], vec![level; n])
    }

    /// A track with one impulse of `level` at frame `at` (of `len`).
    fn impulse(&mut self, level: f32, at: usize, len: usize) -> TrackId {
        let mut v = vec![0.0; len];
        v[at] = level;
        self.track_lr(v.clone(), v)
    }

    fn bus(&mut self) -> TrackId {
        self.s.add_track_of(TrackKind::Bus)
    }

    fn t(&mut self, id: TrackId) -> &mut af2_core::Track {
        self.s.track_mut(id).unwrap()
    }

    fn render(&self, secs: f64) -> AudioBuffer {
        let bufs = self.bufs.clone();
        render(
            mix_program(&self.s, &move |m| bufs.get(&m).cloned(), RATE),
            RATE,
            0.0,
            secs,
        )
    }

    fn mix(&self) -> mix::Mix {
        let bufs = self.bufs.clone();
        mix::build(&self.s, &move |m| bufs.get(&m).cloned(), RATE)
    }
}

fn limiter(look_ms: f32) -> EffectSlot {
    EffectSlot {
        position: None,
        sidechain: None,
        effect: "limiter".into(),
        values: vec![0.0, 0.0, look_ms, 100.0],
        bypass: false,
        data: None,
        ext_id: None,
    }
}

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

fn send(bus: TrackId, db: f32, pan: f32, pre: bool) -> Send {
    Send {
        level_db: db,
        pan,
        pre_fader: pre,
        ..Send::new(bus)
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// The frame of the loudest sample in `x`.
fn peak_at(x: &[f32]) -> usize {
    x.iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0
}

#[test]
fn post_fader_sends_follow_the_fader_not_the_pan_and_pre_fader_ignores_both() {
    for pre in [false, true] {
        let mut f = Fx::new();
        let t = f.steady(0.5, 0.2);
        let bus = f.bus();
        f.t(t).volume_db = -6.0206; // x0.5
        f.t(t).pan = -1.0; // hard left: the right side gone
        f.t(t).output = Some(bus);
        f.t(bus).mute = false;
        // The output goes into the bus too, so take it away with a muted
        // second bus... simpler: send to the bus, output elsewhere.
        let other = f.bus();
        f.t(t).output = Some(other);
        f.t(other).volume_db = -200.0;
        f.t(t).sends.push(send(bus, 0.0, 0.0, pre));
        let out = f.render(0.1);
        let (l, r) = (out.channels[0][400], out.channels[1][400]);
        let want = if pre { 0.5 } else { 0.25 };
        // Both sides: the send has its own (centred) pan.
        assert!(near(l, want) && near(r, want), "pre {pre}: {l} {r}");
    }
}

#[test]
fn sends_pan_and_switch_off() {
    let mut f = Fx::new();
    let t = f.steady(0.5, 0.2);
    let bus = f.bus();
    let sink = f.bus();
    f.t(t).output = Some(sink);
    f.t(sink).mute = true;
    f.t(t).sends.push(send(bus, 0.0, 0.5, false));
    let out = f.render(0.1);
    assert!(near(out.channels[0][400], 0.25) && near(out.channels[1][400], 0.5));
    f.t(t).sends[0].enabled = false;
    let out = f.render(0.1);
    assert!(out.channels[0].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn polarity_flips_and_mono_folds_after_the_chain_and_before_the_pan() {
    let mut f = Fx::new();
    let n = 1600;
    let t = f.track_lr(vec![0.4; n], vec![0.0; n]);
    f.t(t).polarity = true;
    let out = f.render(0.1);
    assert!(near(out.channels[0][400], -0.4));
    f.t(t).polarity = false;
    f.t(t).mono = true;
    // The chain doubles only... the whole sound; the fold comes after it.
    f.t(t).chain.push(amp(6.0206));
    f.t(t).pan = 1.0;
    let out = f.render(0.1);
    // (0.8 + 0) / 2 on both sides, then panned hard right.
    assert!(near(out.channels[0][400], 0.0), "{}", out.channels[0][400]);
    assert!(near(out.channels[1][400], 0.4), "{}", out.channels[1][400]);
}

#[test]
fn balance_pan_keeps_the_centre_at_the_fader_level() {
    let mut f = Fx::new();
    let t = f.steady(0.5, 0.2);
    f.t(t).pan = 0.5;
    let out = f.render(0.1);
    assert!(near(out.channels[0][400], 0.25) && near(out.channels[1][400], 0.5));
}

#[test]
fn soloing_a_bus_plays_what_feeds_it_and_soloing_a_track_plays_its_send_bus() {
    let mut s = Session::new("s", RATE, 0);
    let a = s.add_track();
    s.add_track();
    let bus = s.add_track_of(TrackKind::Bus);
    let verb = s.add_track_of(TrackKind::Bus);
    s.track_mut(a).unwrap().output = Some(bus);
    s.track_mut(a).unwrap().sends.push(Send::new(verb));
    s.track_mut(bus).unwrap().solo = true;
    // The bus and what feeds it; not b. The reverb is downstream of a
    // (heard) only through a, which the forward walk from the bus misses.
    assert_eq!(s.heard(), vec![true, false, true, false]);
    s.track_mut(bus).unwrap().solo = false;
    s.track_mut(a).unwrap().solo = true;
    assert_eq!(s.heard(), vec![true, false, true, true]);
    // A send that is off is not a route.
    s.track_mut(a).unwrap().sends[0].enabled = false;
    assert_eq!(s.heard(), vec![true, false, true, false]);
    // Solo-safe stays heard; mute always wins.
    s.track_mut(verb).unwrap().solo_safe = true;
    s.track_mut(bus).unwrap().mute = true;
    assert_eq!(s.heard(), vec![true, false, false, true]);
}

#[test]
fn track_chains_keep_time_with_look_ahead() {
    let mut f = Fx::new();
    let at = 2000;
    let a = f.impulse(0.25, at, 4000);
    let _b = f.impulse(0.25, at, 4000);
    f.t(a).chain.push(limiter(5.0));
    let m = f.mix();
    assert_eq!(m.lead, 40);
    let out = f.render(0.5);
    // Both impulses land together, on the frame they sit at.
    assert_eq!(peak_at(&out.channels[0]), at);
    assert!(near(out.channels[0][at], 0.5), "{}", out.channels[0][at]);
}

#[test]
fn buses_sum_their_inputs_in_step_with_direct_tracks() {
    let mut f = Fx::new();
    let at = 5500;
    let direct = f.impulse(0.1, at, 8000);
    let bused = f.impulse(0.1, at, 8000);
    let sent = f.impulse(0.1, at, 8000);
    let bus = f.bus();
    f.t(direct).chain.push(limiter(1.0));
    f.t(bus).chain.push(limiter(5.0));
    f.t(bused).output = Some(bus);
    let sink = f.bus();
    f.t(sent).output = Some(sink);
    f.t(sink).volume_db = -200.0;
    f.t(sent).sends.push(send(bus, 0.0, 0.0, true));
    let out = f.render(1.0);
    let x = &out.channels[0];
    assert_eq!(peak_at(x), at);
    assert!(near(x[at], 0.3), "{}", x[at]);
    // Nothing anywhere else.
    let rest: f32 = x
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, v)| v.abs())
        .sum();
    assert!(rest < 1e-4, "{rest}");
}

#[test]
fn a_damaged_loop_still_plays() {
    let mut f = Fx::new();
    let t = f.steady(0.5, 0.2);
    let b1 = f.bus();
    let b2 = f.bus();
    f.t(t).output = Some(b1);
    // A loop only a damaged file can hold: b1 -> b2 -> b1.
    f.t(b1).output = Some(b2);
    f.t(b2).output = Some(b1);
    f.t(b2).sends.push(Send::new(b1));
    let m = f.mix();
    let all: usize = m.levels().iter().map(Vec::len).sum();
    assert_eq!(all, 3);
    // b2's output closes the loop: it goes to the master instead; its
    // send plays nowhere.
    assert_eq!(m.tracks[2].output, None);
    let out = f.render(0.1);
    assert!(near(out.channels[0][400], 0.5), "{}", out.channels[0][400]);
}

#[test]
fn levels_change_live_and_ramp() {
    let mut f = Fx::new();
    f.steady(1.0, 2.0);
    let mut m = f.mix();
    m.parallel = false;
    let dt = 1.0 / RATE as f64;
    let mut out = vec![0.0; BLOCK * 2];
    m.start(0.0);
    m.render(0.0, dt, BLOCK, &mut out, false);
    assert!(near(out[(BLOCK - 1) * 2], 1.0));
    m.set_strip(0, 0.0, 0.0);
    m.render(BLOCK as f64 * dt, dt, BLOCK, &mut out, false);
    // Halfway down the ramp at mid-block, silent at its end.
    assert!((out[BLOCK] - 0.5).abs() < 0.01, "{}", out[BLOCK]);
    assert!(out[(BLOCK - 1) * 2].abs() < 1e-6);
    // No step larger than the ramp allows.
    let steps = out.chunks(2).collect::<Vec<_>>();
    for w in steps.windows(2) {
        assert!((w[1][0] - w[0][0]).abs() <= 1.0 / BLOCK as f32 + 1e-6);
    }
    // The master fader ramps too.
    m.set_strip(0, 1.0, 0.0);
    m.render(2.0 * BLOCK as f64 * dt, dt, BLOCK, &mut out, false);
    m.master = 0.0;
    m.render(3.0 * BLOCK as f64 * dt, dt, BLOCK, &mut out, false);
    assert!((out[BLOCK] - 0.5).abs() < 0.01, "{}", out[BLOCK]);
}

fn lane(target: LaneTarget, pts: &[(f64, f64)]) -> Lane {
    let mut l = Lane::new(target);
    l.points = pts
        .iter()
        .map(|&(time, value)| LanePoint {
            time,
            value,
            curve: Curve::Linear,
        })
        .collect();
    l
}

#[test]
fn a_volume_lane_takes_the_faders_place_and_post_fader_sends_follow_it() {
    let mut f = Fx::new();
    let t = f.steady(0.5, 1.0);
    let bus = f.bus();
    f.t(t).volume_db = -40.0; // set aside while the lane plays
    f.t(t).lanes.push(lane(
        LaneTarget::Volume,
        &[(0.0, 0.0), (0.05, 0.0), (0.5, -6.0206)],
    ));
    f.t(t).sends.push(send(bus, 0.0, 0.0, false));
    let out = f.render(0.6);
    let x = &out.channels[0];
    // Direct and send, both at the lane's level: 0 dB, then -6 dB.
    assert!(near(x[100], 1.0), "{}", x[100]);
    assert!((x[4400] - 0.5).abs() < 1e-3, "{}", x[4400]);
    // A pan lane takes the pan's place on the output only.
    f.t(t).sends.clear();
    f.t(t).lanes = vec![lane(LaneTarget::Pan, &[(0.0, 1.0)])];
    f.t(t).volume_db = 0.0;
    let out = f.render(0.1);
    assert!(near(out.channels[0][400], 0.0) && near(out.channels[1][400], 0.5));
    // A lane switched to live steps aside for its control.
    let mut m = f.mix();
    m.set_lane_live(Some(0), LaneKind::Pan, true);
    let mut o = vec![0.0; BLOCK * 2];
    m.start(0.0);
    m.render(0.0, 1.0 / RATE as f64, BLOCK, &mut o, false);
    assert!(near(o[800], 0.5) && near(o[801], 0.5));
}

#[test]
fn effect_lanes_move_their_setting() {
    let mut f = Fx::new();
    let t = f.steady(0.25, 1.0);
    let mut a = amp(0.0);
    a.ext_id = Some("slot".into());
    f.t(t).chain.push(a);
    f.t(t).lanes.push(lane(
        LaneTarget::Effect {
            slot: "slot".into(),
            param: 0,
            key: None,
        },
        &[(0.0, 0.0), (0.4, 0.0), (0.5, 6.0206)],
    ));
    let out = f.render(0.8);
    let x = &out.channels[0];
    assert!(near(x[800], 0.25), "{}", x[800]);
    assert!((x[5600] - 0.5).abs() < 2e-3, "{}", x[5600]);
}

#[test]
fn the_master_volume_lane_plays() {
    let mut f = Fx::new();
    f.steady(0.5, 1.0);
    f.s.master_lanes
        .push(lane(LaneTarget::Volume, &[(0.0, -6.0206)]));
    let out = f.render(0.1);
    assert!((out.channels[0][400] - 0.25).abs() < 1e-3);
}

#[test]
fn a_key_meets_its_effect_in_step_after_earlier_look_ahead() {
    // Music (0.25) with a 5 ms limiter before a compressor keyed by a
    // voice starting at 1 s, its fader down.
    let mut f = Fx::new();
    let music = f.steady(0.25, 2.0);
    let n = 16000;
    let mut v = vec![0.0; n];
    v[8000..].fill(0.9);
    let voice = f.track_lr(v.clone(), v);
    f.t(voice).volume_db = -200.0;
    f.t(music).chain.push(limiter(5.0));
    f.t(music).chain.push(EffectSlot {
        position: None,
        sidechain: Some(voice),
        effect: "compressor".into(),
        values: vec![-10.0, 10.0, 1.0, 20.0, 0.0, 0.0],
        bypass: false,
        data: None,
        ext_id: None,
    });
    let out = f.render(2.0);
    let x = &out.channels[0];
    // Untouched until the voice starts (not 40 frames early), then ducked.
    assert!(near(x[7990], 0.25), "{}", x[7990]);
    assert!(x[8400] < 0.25 * 0.5, "{}", x[8400]);
}

#[test]
fn one_thread_or_many_give_the_same_mix_with_buses_sends_keys_and_latency() {
    let mut f = Fx::new();
    let a = f.steady(0.3, 2.0);
    let b = f.impulse(0.4, 3000, 16000);
    let c = f.steady(0.2, 2.0);
    let bus = f.bus();
    let verb = f.bus();
    f.t(a).output = Some(bus);
    f.t(b).sends.push(send(verb, -3.0, 0.2, true));
    f.t(c).chain.push(limiter(3.0));
    f.t(c).chain.push(EffectSlot {
        position: None,
        sidechain: Some(b),
        effect: "compressor".into(),
        values: vec![-30.0, 4.0, 1.0, 20.0, 0.0, 0.0],
        bypass: false,
        data: None,
        ext_id: None,
    });
    f.t(bus).chain.push(limiter(2.0));
    let mut serial = f.mix();
    serial.parallel = false;
    let mut par = f.mix();
    let dt = 1.0 / RATE as f64;
    serial.start(0.0);
    par.start(0.0);
    let mut x = vec![0.0; BLOCK * 2];
    let mut y = vec![0.0; BLOCK * 2];
    for k in 0..6 {
        let t0 = k as f64 * BLOCK as f64 * dt;
        serial.render(t0, dt, BLOCK, &mut x, false);
        par.render(t0, dt, BLOCK, &mut y, false);
        assert_eq!(x, y);
    }
}

#[test]
fn midi_clip_level_scales_velocity_and_notes_follow_the_tempo() {
    use af2_core::midi::{plan_notes, ControlEvent, MidiClip, MidiNote, Tempo};
    let note = |start, length, velocity| MidiNote {
        pitch: 60,
        start,
        length,
        velocity,
        channel: 0,
        pan: 0.0,
    };
    let mut c = MidiClip {
        id: 1,
        name: "c".into(),
        start: 4.0,
        length: 4.0,
        notes: vec![note(0.0, 1.0, 127), note(3.5, 2.0, 64), note(4.0, 1.0, 100)],
        timing: Default::default(),
        stretch: 1.0,
        pitch: 0.0,
        muted: false,
        gain_db: -6.0206,
        controls: Vec::new(),
        ext_id: None,
    };
    let tempo = Tempo::constant(120.0, 4, 4);
    let p = plan_notes(std::slice::from_ref(&c), &tempo);
    // The note at the clip's end is not played; the one crossing it is cut.
    assert_eq!(p.len(), 2);
    assert_eq!((p[0].start, p[0].end), (2.0, 2.5));
    assert!((p[0].velocity - 0.5).abs() < 1e-3);
    assert_eq!(p[1].end, 4.0);
    // The pedal holds a released note until it comes up.
    c.controls = vec![
        ControlEvent {
            at: 0.5,
            controller: 64,
            value: 1.0,
            channel: 0,
        },
        ControlEvent {
            at: 2.0,
            controller: 64,
            value: 0.0,
            channel: 0,
        },
    ];
    let p = plan_notes(std::slice::from_ref(&c), &tempo);
    assert_eq!(p[0].end, 3.0);
    c.muted = true;
    assert!(plan_notes(&[c], &tempo).is_empty());
}

/// A mix rebuilt while playing (any edit does this) carries on the running
/// effects: the sound goes on exactly as if nothing had been rebuilt, so
/// the echo's tail and the compressor's envelope do not restart.
#[test]
fn a_rebuilt_mix_keeps_its_effects_running() {
    let mut f = Fx::new();
    let a = f.impulse(0.8, 100, 4000);
    let mut echo = EffectSlot {
        effect: "echo".into(),
        values: vec![120.0, 0.6, 0.5, 6000.0],
        bypass: false,
        position: None,
        sidechain: None,
        data: None,
        ext_id: None,
    };
    echo.values.truncate(4);
    let comp = EffectSlot {
        effect: "compressor".into(),
        values: vec![-30.0, 6.0, 5.0, 200.0, 6.0, 0.0],
        ..echo.clone()
    };
    f.t(a).chain = vec![echo, comp];
    let dt = 1.0 / RATE as f64;
    let n = 256usize;
    let run = |m: &mut mix::Mix, from: usize, blocks: usize, out: &mut Vec<f32>| {
        let mut buf = vec![0.0f32; n * 2];
        for b in 0..blocks {
            let t0 = (from + b * n) as f64 * dt;
            m.render(t0, dt, n, &mut buf, false);
            out.extend_from_slice(&buf);
        }
    };
    // Straight through.
    let mut whole = f.mix();
    whole.start(0.0);
    let mut want = Vec::new();
    run(&mut whole, 0, 20, &mut want);
    // Rebuilt after 4 blocks (the echo's tail is ringing).
    let mut first = f.mix();
    first.start(0.0);
    let mut got = Vec::new();
    run(&mut first, 0, 4, &mut got);
    let mut second = f.mix();
    assert!(second.adopt(&mut first));
    run(&mut second, 4 * n, 16, &mut got);
    assert_eq!(want.len(), got.len());
    let worst = want
        .iter()
        .zip(&got)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "rebuilt mix differs by {worst}");
    assert!(
        want.iter().skip(4 * n * 2).any(|v| v.abs() > 1e-3),
        "the tail rings"
    );
}
