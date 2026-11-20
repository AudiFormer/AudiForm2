// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Tests for what a clip carries itself: its automation (volume, pan,
//! effect values), its own effect rack and its Source Channels, live and
//! offline.

use std::collections::HashMap;
use std::sync::Arc;

use af27_core::automation::{Curve, Lane, LanePoint, LaneTarget};
use af27_core::session::{ChannelSend, Clip, MediaItem};
use af27_core::{AudioBuffer, EffectSlot, MediaId, Session, TrackKind};

use crate::mix::{self, BLOCK};
use crate::{mix_program, render};

const RATE: u32 = 8000;

struct Fx {
    s: Session,
    bufs: HashMap<MediaId, Arc<AudioBuffer>>,
}

impl Fx {
    /// One track with one clip at `start` playing `left` and `right`.
    fn new(left: Vec<f32>, right: Vec<f32>, start: f64) -> Fx {
        let mut s = Session::new("clip fx", RATE, 0);
        let t = s.add_track_of(TrackKind::Audio);
        let m = s.alloc_id();
        let len = left.len() as f64 / RATE as f64;
        s.media.push(MediaItem {
            id: m,
            name: "m".into(),
            path: None,
            duration: len,
            rate: RATE,
            channels: 2,
            ext_id: None,
        });
        let mut bufs = HashMap::new();
        bufs.insert(
            m,
            Arc::new(AudioBuffer {
                rate: RATE,
                channels: vec![left, right],
            }),
        );
        let id = s.alloc_id();
        s.track_mut(t).unwrap().clips.push(Clip {
            props: Default::default(),
            id,
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
        Fx { s, bufs }
    }

    fn steady(level: f32, secs: f64, start: f64) -> Fx {
        let n = (secs * RATE as f64) as usize;
        Fx::new(vec![level; n], vec![level; n], start)
    }

    fn clip(&mut self) -> &mut Clip {
        &mut self.s.tracks[0].clips[0]
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

fn pt(time: f64, value: f64) -> LanePoint {
    LanePoint {
        time,
        value,
        curve: Curve::Linear,
    }
}

fn lane(target: LaneTarget, points: Vec<LanePoint>) -> Lane {
    let mut l = Lane::new(target);
    l.points = points;
    l
}

fn slot(effect: &str, values: Vec<f32>) -> EffectSlot {
    EffectSlot {
        position: None,
        sidechain: None,
        effect: effect.into(),
        values,
        bypass: false,
        data: None,
        ext_id: None,
    }
}

fn at(b: &AudioBuffer, secs: f64) -> (f32, f32) {
    let i = (secs * RATE as f64).round() as usize;
    (b.channels[0][i], b.channels[1][i])
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 2e-3
}

#[test]
fn a_clip_volume_line_plays_and_moves_with_the_clip() {
    // Half a second in, a clip a second long: 0 dB falling to -12 dB
    // across its second half, in clip time.
    let mut f = Fx::steady(0.5, 1.0, 0.5);
    f.clip().props.lanes = vec![lane(
        LaneTarget::Volume,
        vec![pt(0.5, 0.0), pt(1.0, -12.0412)],
    )];
    let out = f.render(1.6);
    assert_eq!(at(&out, 0.25), (0.0, 0.0), "before the clip");
    assert!(
        near(at(&out, 0.75).0, 0.5),
        "0 dB held before the first point"
    );
    // 0.75 s into the clip: -6 dB.
    assert!(near(at(&out, 1.25).0, 0.25), "{:?}", at(&out, 1.25));
    assert!(near(at(&out, 1.25).1, 0.25));
    // The clip moved: its line comes along.
    f.clip().start = 0.0;
    let moved = f.render(1.1);
    assert!(near(at(&moved, 0.75).0, 0.25));
    // At or below silence the line mutes.
    f.clip().props.lanes[0].points[1].value = -80.0;
    let quiet = f.render(1.1);
    assert!(at(&quiet, 0.999).0.abs() < 1e-3);
}

#[test]
fn a_clip_pan_line_and_a_hold_point() {
    let mut f = Fx::steady(0.5, 1.0, 0.0);
    let mut hold = pt(0.0, -1.0);
    hold.curve = Curve::Hold;
    f.clip().props.lanes = vec![lane(LaneTarget::Pan, vec![hold, pt(0.5, 1.0)])];
    let out = f.render(1.0);
    // Held hard left until the next point, then hard right.
    assert_eq!(at(&out, 0.25), (0.5, 0.0));
    assert_eq!(at(&out, 0.75), (0.0, 0.5));
}

#[test]
fn source_channels_pick_swap_and_flip_sides() {
    let n = RATE as usize;
    let mut f = Fx::new(vec![0.2; n], vec![0.6; n], 0.0);
    let plain = f.render(0.5);
    assert_eq!(at(&plain, 0.25), (0.2, 0.6));
    // Only the right side, on both sides, the right one flipped.
    f.clip().props.channel_map = Some(vec![
        ChannelSend {
            left: 0.0,
            right: 0.0,
        },
        ChannelSend {
            left: 1.0,
            right: -1.0,
        },
    ]);
    let out = f.render(0.5);
    let (l, r) = at(&out, 0.25);
    assert!(near(l, 0.6) && near(r, -0.6), "{l} {r}");
    // Swapped at half level; a map shorter than the file leaves the
    // rest silent.
    f.clip().props.channel_map = Some(vec![ChannelSend {
        left: 0.0,
        right: 0.5,
    }]);
    let out = f.render(0.5);
    let (l, r) = at(&out, 0.25);
    assert!(near(l, 0.0) && near(r, 0.1), "{l} {r}");
}

#[test]
fn a_clip_rack_runs_before_the_track_rack_live_and_offline() {
    let mut f = Fx::steady(0.5, 1.0, 0.0);
    f.clip().props.chain = vec![slot("amplify", vec![-6.0206])];
    f.s.tracks[0].chain = vec![slot("amplify", vec![-6.0206])];
    let out = f.render(1.0);
    assert!(near(at(&out, 0.5).0, 0.125), "{:?}", at(&out, 0.5));
    // The rack's own controls: half wet, then output gain, then off.
    f.clip().props.rack.mix = 0.5;
    let out = f.render(1.0);
    assert!(near(at(&out, 0.5).0, (0.25 * 0.5 + 0.5 * 0.5) * 0.5));
    f.clip().props.rack.mix = 1.0;
    f.clip().props.rack.bypass = true;
    let out = f.render(1.0);
    assert!(near(at(&out, 0.5).0, 0.25));
    f.clip().props.rack.bypass = false;

    // Live: new settings, bypass and rack levels reach the playing mix.
    let id = f.s.tracks[0].clips[0].id;
    let mut m = f.mix();
    let dt = 1.0 / RATE as f64;
    let mut block = vec![0.0f32; BLOCK * 2];
    let mut play = |m: &mut mix::Mix, t: f64| {
        m.render(t, dt, 64, &mut block, false);
        block[0]
    };
    m.start(0.0);
    assert!(near(play(&mut m, 0.1), 0.125));
    m.clip_fx(id).unwrap().set(0, &[0.0]);
    assert!(near(play(&mut m, 0.2), 0.25));
    m.clip_fx(id).unwrap().chain.set_bypass(0, true);
    m.clip_fx(id).unwrap().set(0, &[-12.0]);
    assert!(near(play(&mut m, 0.3), 0.25));
    let rack = af27_core::Rack {
        output_db: -6.0206,
        ..Default::default()
    };
    m.clip_fx(id).unwrap().chain.set_rack(&rack);
    let meters = m.clip_meters();
    meters[0].1.take();
    assert!(near(play(&mut m, 0.4), 0.125));
    // Its meters read the clip rack (the track halves it again after).
    assert_eq!(meters.len(), 1);
    assert_eq!(meters[0].0, id);
    let (input, output) = meters[0].1.take();
    assert!(near(input, 0.5) && near(output, 0.25), "{input} {output}");
}

#[test]
fn a_look_ahead_clip_effect_stays_in_step_and_ends_with_the_clip() {
    // An impulse 0.2 s in; the clip's limiter looks 5 ms ahead.
    let n = RATE as usize / 2;
    let mut v = vec![0.0; n];
    v[1600] = 0.5;
    let mut f = Fx::new(v.clone(), v, 0.25);
    f.clip().props.chain = vec![slot("limiter", vec![0.0, 0.0, 5.0, 100.0])];
    let out = f.render(1.0);
    let peak = out.channels[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    assert_eq!(peak.0, 2000 + 1600, "the impulse stays where it was");
    assert!(near(*peak.1, 0.5));
    // Nothing before or after the clip.
    let end = 2000 + n;
    assert!(out.channels[0][..2000].iter().all(|x| *x == 0.0));
    assert!(out.channels[0][end..].iter().all(|x| *x == 0.0));
}

#[test]
fn a_clip_effect_lane_moves_its_value() {
    let mut f = Fx::steady(0.5, 1.0, 0.0);
    let mut s = slot("amplify", vec![0.0]);
    s.ext_id = Some("amp".into());
    f.clip().props.chain = vec![s];
    let mut step = pt(0.0, 0.0);
    step.curve = Curve::Hold;
    f.clip().props.lanes = vec![lane(
        LaneTarget::Effect {
            slot: "amp".into(),
            param: 0,
            key: None,
        },
        vec![step, pt(0.5, -6.0206)],
    )];
    let out = f.render(1.0);
    assert!(near(at(&out, 0.25).0, 0.5));
    assert!(near(at(&out, 0.75).0, 0.25), "{:?}", at(&out, 0.75));
}
