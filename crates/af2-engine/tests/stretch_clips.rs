// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Clip stretch, pitch, reverse and polarity, through the mix: offline
//! renders and the live path (before and after the background work).

use std::f64::consts::PI;
use std::sync::Arc;

use af2_core::session::{StretchKind, StretchMode};
use af2_core::{AudioBuffer, MediaItem, Session};
use af2_engine::{mix, mix_program, render, BLOCK};

const RATE: u32 = 48_000;

fn sine(f: f64, secs: f64) -> AudioBuffer {
    let n = (secs * RATE as f64) as usize;
    let mut b = AudioBuffer::new(RATE, 1, n);
    for (i, s) in b.channels[0].iter_mut().enumerate() {
        *s = ((2.0 * PI * f * i as f64 / RATE as f64).sin() * 0.5) as f32;
    }
    b
}

/// A session with one clip of all of `buf` at 0.25 s, `edit` applied.
fn session(buf: &AudioBuffer, edit: impl Fn(&mut af2_core::Clip)) -> Session {
    let mut s = Session::new("t", RATE, 1);
    let id = s.alloc_id();
    s.media.push(MediaItem {
        id,
        name: "m".into(),
        path: None,
        duration: buf.frames() as f64 / RATE as f64,
        rate: RATE,
        channels: buf.num_channels() as u16,
        ext_id: None,
    });
    let t = s.tracks[0].id;
    s.add_clip(t, id, 0.25).unwrap();
    edit(&mut s.tracks[0].clips[0]);
    s
}

fn mixdown(s: &Session, buf: &Arc<AudioBuffer>, secs: f64) -> Vec<f32> {
    let b = buf.clone();
    let p = mix_program(s, &move |_| Some(b.clone()), RATE);
    render(p, RATE, 0.0, secs).channels[0].clone()
}

/// First and last frame above a small level.
fn extent(x: &[f32]) -> (usize, usize) {
    let first = x.iter().position(|v| v.abs() > 1e-3).unwrap();
    let last = x.iter().rposition(|v| v.abs() > 1e-3).unwrap();
    (first, last)
}

fn peak_hz(x: &[f32]) -> f64 {
    let n = 16_384;
    let s = (x.len() - n) / 2;
    let mut re: Vec<f64> = (0..n)
        .map(|i| x[s + i] as f64 * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()))
        .collect();
    let mut im = vec![0.0; n];
    af2_dsp::fft::fft(&mut re, &mut im, false);
    let m: Vec<f64> = (0..n / 2).map(|b| re[b].hypot(im[b])).collect();
    let b = (1..n / 2 - 1)
        .max_by(|a, b| m[*a].partial_cmp(&m[*b]).unwrap())
        .unwrap();
    let (a, c, d) = (m[b - 1].ln(), m[b].ln(), m[b + 1].ln());
    (b as f64 + 0.5 * (a - d) / (a - 2.0 * c + d)) * RATE as f64 / n as f64
}

#[test]
fn ratio_two_plays_twice_as_long_at_the_same_pitch() {
    let buf = Arc::new(sine(440.0, 1.0));
    for kind in [StretchKind::Monophonic, StretchKind::Polyphonic] {
        let s = session(&buf, |c| {
            c.props.stretch.mode = StretchMode::Rendered;
            c.props.stretch.kind = kind;
            c.props.stretch.ratio = 2.0;
            c.length *= 2.0;
        });
        let y = mixdown(&s, &buf, 3.0);
        let (a, b) = extent(&y);
        let len = (b + 1 - a) as i64;
        assert!(
            (len - 96_000).abs() <= BLOCK as i64,
            "{kind:?}: {len} frames"
        );
        assert!(
            (a as i64 - 12_000).abs() <= BLOCK as i64,
            "{kind:?}: starts {a}"
        );
        let f = peak_hz(&y[a..=b]);
        assert!((f - 440.0).abs() < 2.0, "{kind:?}: {f} Hz");
    }
}

#[test]
fn pitch_shift_moves_a_sine() {
    let buf = Arc::new(sine(440.0, 1.0));
    for (st, want) in [(12.0f32, 880.0), (-7.0, 440.0 / 1.4983)] {
        let s = session(&buf, |c| {
            c.props.stretch.mode = StretchMode::Live;
            c.props.stretch.pitch = st;
        });
        let y = mixdown(&s, &buf, 1.5);
        let (a, b) = extent(&y);
        assert!(((b + 1 - a) as i64 - 48_000).abs() <= BLOCK as i64);
        let f = peak_hz(&y[a..=b]);
        assert!((f / want - 1.0).abs() < 0.01, "{st}: {f} Hz, want {want}");
    }
}

#[test]
fn varispeed_halves_speed_and_pitch() {
    let buf = Arc::new(sine(440.0, 1.0));
    let s = session(&buf, |c| {
        c.props.stretch.mode = StretchMode::Live;
        c.props.stretch.kind = StretchKind::Varispeed;
        c.props.stretch.ratio = 2.0;
        c.length *= 2.0;
    });
    let y = mixdown(&s, &buf, 3.0);
    let (a, b) = extent(&y);
    assert!(((b + 1 - a) as i64 - 96_000).abs() <= BLOCK as i64);
    let f = peak_hz(&y[a..=b]);
    assert!((f - 220.0).abs() < 1.0, "{f} Hz");
}

#[test]
fn reverse_and_invert() {
    // A rising ramp, part of it used.
    let n = 24_000;
    let mut b = AudioBuffer::new(RATE, 1, n);
    for (i, s) in b.channels[0].iter_mut().enumerate() {
        *s = (i + 1) as f32 / n as f32;
    }
    let buf = Arc::new(b);
    let plain = mixdown(&session(&buf, |_| {}), &buf, 1.0);
    let rev = mixdown(&session(&buf, |c| c.props.reversed = true), &buf, 1.0);
    let inv = mixdown(&session(&buf, |c| c.props.inverted = true), &buf, 1.0);
    let at = 12_000; // the clip starts at 0.25 s
                     // (The ramp rises 1/24000 a frame: a frame's slack at the edges,
                     // where the clip starts between frames, is within the tolerance.)
    for i in 1..n - 1 {
        assert!(
            (rev[at + i] - plain[at + n - 1 - i]).abs() < 1e-4,
            "frame {i}"
        );
        assert_eq!(inv[at + i], -plain[at + i]);
    }
    // A trimmed, reversed clip plays its own span backwards.
    let trimmed = mixdown(
        &session(&buf, |c| {
            c.offset = 0.1;
            c.length = 0.2;
            c.props.reversed = true;
        }),
        &buf,
        1.0,
    );
    assert!((trimmed[at + 1] - buf.channels[0][14_398]).abs() < 1e-4);
    assert!((trimmed[at + 9_598] - buf.channels[0][4_801]).abs() < 1e-4);
    // Reversed and stretched: the prepared copy runs high to low too.
    let rs = mixdown(
        &session(&buf, |c| {
            c.props.reversed = true;
            c.props.stretch.mode = StretchMode::Live;
            c.props.stretch.kind = StretchKind::Varispeed;
            c.props.stretch.ratio = 2.0;
            c.length *= 2.0;
        }),
        &buf,
        1.5,
    );
    assert!(rs[at + 1_000] > 0.9 && rs[at + 46_000] < 0.1);
}

#[test]
fn live_mix_plays_tape_speed_until_ready_then_the_stretch() {
    let buf = Arc::new(sine(440.0, 1.0));
    let s = session(&buf, |c| {
        c.start = 0.0;
        c.props.stretch.mode = StretchMode::Live;
        c.props.stretch.ratio = 2.0;
        c.props.stretch.window_ms = 31.0; // its own cache entry
        c.length *= 2.0;
    });
    let b = buf.clone();
    let mut m = mix::build(&s, &move |_| Some(b.clone()), RATE);
    m.parallel = false;
    let prepared = m.tracks[0].clips[0].prepared.clone().unwrap();
    let dt = 1.0 / RATE as f64;
    // Mid-clip, wherever the background work is, it sounds (no silence,
    // no glitch at the jump).
    let mut out = vec![0.0f32; BLOCK * 2];
    m.render(1.0, dt, BLOCK, &mut out, false);
    assert!(out.iter().any(|v| v.abs() > 0.1));
    prepared.ensure();
    let v0 = af2_engine::stretch::version();
    assert!(v0 > 0);
    let mut seen = Vec::new();
    for k in 0..16 {
        m.render(0.5 + (k * BLOCK) as f64 * dt, dt, BLOCK, &mut out, false);
        seen.extend(out.iter().step_by(2).copied());
    }
    let f = peak_hz(&seen);
    assert!((f - 440.0).abs() < 2.0, "{f} Hz");
}
