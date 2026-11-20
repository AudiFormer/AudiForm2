// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! Tests for 3D Space in the mix: placing on the bed, gliding moves, the
//! LFE send and band, 3D Buses, multichannel clips, spot automation.

use std::collections::HashMap;
use std::sync::Arc;

use af27_core::session::{Clip, MediaItem};
use af27_core::space::{LaneCurve, LanePoint, SpaceLane, SpaceLayout, SpaceParam, SpacePos};
use af27_core::{AudioBuffer, EffectSlot, MediaId, Session, TrackId, TrackKind};

use crate::{mix_program, render, render_bed, Program};

const RATE: u32 = 8000;

struct Fixture {
    s: Session,
    bufs: HashMap<MediaId, Arc<AudioBuffer>>,
}

impl Fixture {
    fn new(layout: Option<SpaceLayout>) -> Fixture {
        let mut s = Session::new("space", RATE, 0);
        s.space = layout;
        Fixture {
            s,
            bufs: HashMap::new(),
        }
    }

    /// A track playing `chans` (one Vec per channel) from 0.
    fn track(&mut self, chans: Vec<Vec<f32>>) -> TrackId {
        let id = self.s.add_track();
        let m = self.s.alloc_id();
        let frames = chans[0].len();
        let len = frames as f64 / RATE as f64;
        self.s.media.push(MediaItem {
            id: m,
            name: "m".into(),
            path: None,
            duration: len,
            rate: RATE,
            channels: chans.len() as u16,
            ext_id: None,
        });
        self.bufs.insert(
            m,
            Arc::new(AudioBuffer {
                rate: RATE,
                channels: chans,
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

    fn dc(&mut self, secs: f64) -> TrackId {
        self.track(vec![vec![1.0; (secs * RATE as f64) as usize]])
    }

    fn t(&mut self, id: TrackId) -> &mut af27_core::Track {
        self.s.track_mut(id).unwrap()
    }

    fn program(&self) -> Program {
        let bufs = self.bufs.clone();
        mix_program(&self.s, &move |m| bufs.get(&m).cloned(), RATE)
    }

    fn bed(&self, secs: f64) -> AudioBuffer {
        render_bed(self.program(), RATE, 0.0, secs)
    }
}

fn tone(hz: f64, secs: f64) -> Vec<f32> {
    (0..(secs * RATE as f64) as usize)
        .map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / RATE as f64).sin() as f32)
        .collect()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |a, v| a.max(v.abs()))
}

#[test]
fn a_mono_click_at_front_left_lands_only_on_left() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let mut click = vec![0.0f32; 800];
    click[100] = 1.0;
    let id = f.track(vec![click]);
    f.t(id).pos = SpacePos {
        x: -1.0,
        y: 1.0,
        ..SpacePos::default()
    };
    let bed = f.bed(0.1);
    assert!((bed.channels[0][100] - 1.0).abs() < 1e-5);
    for c in 1..6 {
        assert!(peak(&bed.channels[c]) < 1e-6, "speaker {c}");
    }
}

#[test]
fn a_moving_source_glides_with_no_jump() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let id = f.dc(1.0);
    f.t(id).pos.x = -1.0;
    let Program::Mix(mut m) = f.program() else {
        unreachable!()
    };
    let n = crate::BLOCK;
    let dt = 1.0 / RATE as f64;
    let mut out = vec![0.0f32; n * 6];
    m.render(0.0, dt, n, &mut out, true);
    let before = out[(n - 1) * 6];
    assert!((before - 1.0).abs() < 1e-5);
    // Jump the spot to the centre between blocks.
    m.set_space(
        0,
        SpacePos {
            x: 0.0,
            ..SpacePos::default()
        },
    );
    m.render(n as f64 * dt, dt, n, &mut out, true);
    let mut last = before;
    for i in 0..n {
        let v = out[i * 6];
        assert!((v - last).abs() <= 1.0 / n as f32 + 1e-5, "step at {i}");
        last = v;
    }
    assert!(last.abs() < 1e-5 && (out[(n - 1) * 6 + 2] - 1.0).abs() < 1e-5);
}

#[test]
fn an_automated_sweep_keeps_its_power() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround71));
    let id = f.dc(2.0);
    let lane = |param, a, b| SpaceLane {
        param,
        points: vec![
            LanePoint {
                t: 0.0,
                v: a,
                curve: LaneCurve::Linear,
            },
            LanePoint {
                t: 2.0,
                v: b,
                curve: LaneCurve::Linear,
            },
        ],
        enabled: true,
    };
    f.t(id).spatial.lanes = vec![
        lane(SpaceParam::X, -1.0, 1.0),
        lane(SpaceParam::Y, 1.0, -1.0),
    ];
    let bed = f.bed(2.0);
    let frames = bed.frames();
    for i in (0..frames).step_by(37) {
        let p: f32 = bed.channels.iter().map(|c| c[i] * c[i]).sum();
        assert!(p > 0.7 && p < 1.05, "frame {i}: power {p}");
    }
    // It went from front left to back right.
    assert!(bed.channels[0][10] > 0.9);
    assert!(bed.channels[5][frames - 10] > 0.9, "rear right");
}

#[test]
fn a_hold_point_holds_the_spot() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let id = f.dc(1.0);
    f.t(id).spatial.lanes = vec![SpaceLane {
        param: SpaceParam::X,
        points: vec![
            LanePoint {
                t: 0.0,
                v: -1.0,
                curve: LaneCurve::Hold,
            },
            LanePoint {
                t: 0.9,
                v: 1.0,
                curve: LaneCurve::Linear,
            },
        ],
        enabled: true,
    }];
    let bed = f.bed(1.0);
    assert!((bed.channels[0][RATE as usize / 2] - 1.0).abs() < 1e-5);
}

#[test]
fn the_lfe_send_keeps_only_the_sub_band() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let lo = f.track(vec![tone(50.0, 2.0)]);
    f.t(lo).pos.lfe = 1.0;
    let bed = f.bed(2.0);
    let tail = RATE as usize..2 * RATE as usize;
    let lfe = peak(&bed.channels[3][tail.clone()]);
    assert!((lfe - 1.0).abs() < 0.1, "50 Hz: {lfe}");
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let hi = f.track(vec![tone(1000.0, 2.0)]);
    f.t(hi).pos.lfe = 1.0;
    let bed = f.bed(2.0);
    assert!(peak(&bed.channels[3][tail]) < 0.01);
    // A placed sound never reaches the LFE without a send.
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    f.track(vec![tone(50.0, 1.0)]);
    assert!(peak(&f.bed(1.0).channels[3]) < 1e-6);
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

#[test]
fn a_3d_bus_runs_its_effects_on_its_own_bed() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround714));
    let src = f.dc(0.5);
    let bus = f.s.add_track_of(TrackKind::Bus);
    f.t(bus).spatial.room = true;
    f.t(bus).chain.push(amp(-6.0206));
    // The bus's own spot is ignored: its inputs are placed.
    f.t(bus).pos.x = 1.0;
    f.t(src).output = Some(bus);
    f.t(src).pos = SpacePos {
        x: -1.0,
        y: -1.0,
        z: 1.0,
        ..SpacePos::default()
    };
    let bed = f.bed(0.5);
    // Top rear left at half level, nothing elsewhere.
    assert!(
        (bed.channels[10][100] - 0.5).abs() < 1e-3,
        "{}",
        bed.channels[10][100]
    );
    for c in (0..12).filter(|c| *c != 10) {
        assert!(peak(&bed.channels[c]) < 1e-5, "speaker {c}");
    }
    // A send into it is placed by the source's spot too.
    f.t(src).output = None;
    f.t(src).sends.push(af27_core::Send {
        bus,
        level_db: 0.0,
        pre_fader: false,
        ext_id: None,
    });
    let bed = f.bed(0.5);
    assert!(
        (bed.channels[10][100] - 1.5).abs() < 1e-3,
        "direct plus half from the bus"
    );
    // Outside 3D Space it is a plain stereo bus.
    f.s.space = None;
    let st = render(f.program(), RATE, 0.0, 0.5);
    assert!(peak(&st.channels[0]) > 0.5);
}

#[test]
fn a_5_1_clip_in_7_1_4_plays_from_its_speakers() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround714));
    let chans: Vec<Vec<f32>> = (0..6).map(|k| vec![0.1 * (k + 1) as f32; 800]).collect();
    let id = f.track(chans);
    // Its spot does not matter: the channels go to speakers.
    f.t(id).pos.x = 1.0;
    let bed = f.bed(0.1);
    let at = |c: usize| bed.channels[c][400];
    for (c, want) in [(0, 0.1), (1, 0.2), (2, 0.3), (4, 0.5), (5, 0.6)] {
        assert!((at(c) - want).abs() < 1e-4, "speaker {c}: {}", at(c));
    }
    // The LFE channel is on the LFE (inside its band filter: DC is cut).
    assert!(peak(&bed.channels[3]) > 0.01);
    for c in 6..12 {
        assert!(at(c).abs() < 1e-6, "speaker {c}");
    }
    // Folded to stereo when the file had its own channel mix.
    f.t(id).clips[0].props.fold_channels = true;
    let bed = f.bed(0.1);
    // Placed as a stereo source at the right of the front: nothing behind.
    assert!(bed.channels[1][400] > 0.05);
    assert!(bed.channels[4][400].abs() < 1e-6 && bed.channels[5][400].abs() < 1e-6);
}

#[test]
fn spread_reaches_the_back_of_the_room() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let id = f.track(vec![tone(440.0, 1.0)]);
    f.t(id).pos.spread = 0.5;
    let bed = f.bed(1.0);
    let half = RATE as usize / 2;
    assert!(peak(&bed.channels[4][half..]) > 0.1);
    assert!(peak(&bed.channels[5][half..]) > 0.1);
    assert!(peak(&bed.channels[3]) < 1e-6);
}

#[test]
fn stereo_listening_folds_the_bed() {
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    f.s.monitor = af27_core::space::Monitor::StereoFold;
    let id = f.dc(0.5);
    f.t(id).pos.x = -1.0;
    let st = render(f.program(), RATE, 0.0, 0.5);
    assert!((st.channels[0][100] - 1.0).abs() < 1e-5);
    assert!(st.channels[1][100].abs() < 1e-6);
}

#[test]
fn bed_exports_have_their_layout_channels_and_order() {
    for (layout, n) in [
        (SpaceLayout::Stereo20, 2),
        (SpaceLayout::Surround51, 6),
        (SpaceLayout::Surround71, 8),
        (SpaceLayout::Surround512, 8),
        (SpaceLayout::Surround714, 12),
    ] {
        let mut f = Fixture::new(Some(layout));
        f.dc(0.1);
        assert_eq!(f.bed(0.1).num_channels(), n, "{layout:?}");
    }
    // Film order: a source on the left side wall lands on channel 4.
    let mut f = Fixture::new(Some(SpaceLayout::Surround714));
    let id = f.dc(0.1);
    f.t(id).pos = SpacePos {
        x: -1.0,
        y: 0.0,
        ..SpacePos::default()
    };
    assert!((f.bed(0.1).channels[6][100] - 1.0).abs() < 1e-5);
    let mut p = f.program();
    if let Program::Mix(m) = &mut p {
        m.set_bed_order(af27_space::pan::film_order(SpaceLayout::Surround714));
    }
    let film = render_bed(p, RATE, 0.0, 0.1);
    assert!((film.channels[4][100] - 1.0).abs() < 1e-5);
    assert!(film.channels[6][100].abs() < 1e-6);
}

#[test]
fn place_elements_lines_up_with_everything_else() {
    use af27_core::space::SpatializeMode;
    let rate = RATE as usize;
    let mut click = vec![0.0f32; rate];
    click[3000] = 1.0;
    // A session with one steered and one plain track: both clicks land
    // on the frame they were placed at, as without Place Elements.
    let mut f = Fixture::new(Some(SpaceLayout::Surround51));
    let a = f.track(vec![click.clone(), click.clone()]);
    f.t(a).pos.spatialize = 1.0;
    f.t(a).pos.spatialize_mode = SpatializeMode::Steer;
    let b = f.track(vec![click.clone()]);
    f.t(b).pos.x = -1.0;
    let Program::Mix(m) = f.program() else {
        unreachable!()
    };
    assert_eq!(m.lead(), af27_space::steer::STEER_FRAME);
    let bed = f.bed(1.0);
    let peak_at = |c: usize| {
        (0..bed.frames())
            .max_by(|&i, &j| {
                bed.channels[c][i]
                    .abs()
                    .total_cmp(&bed.channels[c][j].abs())
            })
            .unwrap()
    };
    // The plain track on L, the steered one (centred) on C.
    assert_eq!(peak_at(0), 3000);
    assert_eq!(peak_at(2), 3000);
    // A render from part way in still has what comes right after its start.
    let from = 3000.0 / RATE as f64 - 0.01;
    let part = render_bed(f.program(), RATE, from, from + 0.1);
    let at = (0.01 * RATE as f64).round() as usize;
    // (The steered track spills a little onto L around its click.)
    assert!(
        (part.channels[0][at] - 1.0).abs() < 0.06,
        "{}",
        part.channels[0][at]
    );
    // Without a steered track there is no delay.
    f.t(a).pos.spatialize_mode = SpatializeMode::SideSpread;
    let Program::Mix(m) = f.program() else {
        unreachable!()
    };
    assert_eq!(m.lead(), 0);
}
