// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! A frame-time bench for zooming and scrolling a large session, run by
//! hand: `cargo test --release -p audiform2 zoom_bench -- --ignored
//! --nocapture`. It builds a 32-track session of synthetic audio, draws
//! the Multitrack headless (layout plus tessellation, the CPU side of a
//! frame) while the view zooms in and out and scrolls, and prints the
//! frame times. It reads the user's settings (theme and the like) but
//! writes nothing.

#![cfg(test)]

use std::time::Instant;

use af27_core::AudioBuffer;

use crate::state::{Mode, State, View};

fn synth(secs: f64, seed: u32) -> AudioBuffer {
    let rate = 48_000;
    let n = (secs * rate as f64) as usize;
    let mut b = AudioBuffer::new(rate, 2, n);
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    for ch in 0..2 {
        for (i, s) in b.channels[ch].iter_mut().enumerate() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let env = 0.5 + 0.5 * ((i as f32 / rate as f32) * 0.7 + ch as f32).sin();
            *s = noise * env * 0.6;
        }
    }
    b
}

fn session(tracks: usize, track_h: f32) -> State {
    let mut st = State::new();
    st.mode = Mode::Multitrack;
    // Four 3-minute stereo files shared by the clips.
    let docs: Vec<_> = (0..4)
        .map(|k| st.add_doc(format!("bench{k}.wav"), None, synth(180.0, k + 1)))
        .collect();
    while st.session.tracks.len() < tracks {
        st.session.add_track();
    }
    st.session.tracks.truncate(tracks);
    let ids: Vec<_> = st.session.tracks.iter().map(|t| t.id).collect();
    for (ti, id) in ids.iter().enumerate() {
        let clips = if std::env::var_os("AF2_BENCH_NOCLIPS").is_some() {
            0
        } else {
            4
        };
        for k in 0..clips {
            let m = st.media_for_doc(docs[(ti + k) % 4]).unwrap();
            st.session
                .add_clip(*id, m, k as f64 * 185.0 + (ti % 3) as f64);
        }
    }
    for t in &mut st.session.tracks {
        t.height = track_h;
    }
    st.session_view = View::fit(st.session.duration());
    st
}

fn frame(ctx: &egui::Context, st: &mut State, caches: &mut crate::ui::Caches) -> f64 {
    let t0 = Instant::now();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1920.0, 1080.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::multitrack::editor(ui, st, caches);
        });
    });
    let t1 = Instant::now();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    if std::env::var_os("AF2_BENCH_SPLIT").is_some() {
        let v: usize = prims
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                _ => 0,
            })
            .sum();
        eprintln!(
            "layout {:.2} ms, tessellate {:.2} ms, {v} vertices",
            (t1 - t0).as_secs_f64() * 1000.0,
            t1.elapsed().as_secs_f64() * 1000.0
        );
    }
    std::hint::black_box(prims);
    t0.elapsed().as_secs_f64() * 1000.0
}

fn report(name: &str, mut ms: Vec<f64>) {
    ms.sort_by(|a, b| a.total_cmp(b));
    let n = ms.len();
    let mean = ms.iter().sum::<f64>() / n as f64;
    eprintln!(
        "{name:<44} mean {mean:6.2} ms  median {:6.2}  p95 {:6.2}  max {:6.2}",
        ms[n / 2],
        ms[(n * 95 / 100).min(n - 1)],
        ms[n - 1]
    );
}

/// `AF2_BENCH_WRITE=<folder>`: writes the bench session (four WAV files
/// and `bench32.af27`) there, to open in the app.
#[test]
#[ignore]
fn zoom_bench_write_session() {
    let Some(dir) = std::env::var_os("AF2_BENCH_WRITE").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let mut st = session(32, 84.0);
    for (k, m) in st.session.media.iter_mut().enumerate() {
        let path = dir.join(format!("bench{k}.wav"));
        let buf = synth(180.0, k as u32 + 1);
        af27_io::write_wav(&path, &buf, af27_io::WavFormat::Pcm16).unwrap();
        m.path = Some(path);
    }
    std::fs::write(dir.join("bench32.af27"), st.session.to_json()).unwrap();
}

#[test]
#[ignore]
fn zoom_bench() {
    for (label, h) in [
        ("84 px tracks", 84.0),
        ("40 px tracks (most in view)", 40.0),
    ] {
        let mut st = session(32, h);
        let ctx = egui::Context::default();
        let mut caches = crate::ui::Caches::default();
        for _ in 0..5 {
            frame(&ctx, &mut st, &mut caches);
        }
        let lim = st.session_limits();
        let full = st.session_view;
        // Zoom from the whole session to close in, and back, one wheel
        // notch per frame at a fixed spot.
        let mut ms = Vec::new();
        let anchor = full.start + full.len * 0.37;
        for i in 0..120 {
            let f = if i < 60 { 0.8 } else { 1.25 };
            st.session_view.zoom_at(f, anchor, &lim);
            ms.push(frame(&ctx, &mut st, &mut caches));
        }
        report(&format!("{label}: wheel zoom"), ms);
        // Scroll sideways at a mid zoom.
        st.session_view = View {
            start: 0.0,
            len: 60.0,
        };
        let mut ms = Vec::new();
        for _ in 0..120 {
            st.session_view.pan_px(25.0, st.session_px as f64, &lim);
            ms.push(frame(&ctx, &mut st, &mut caches));
        }
        report(&format!("{label}: scroll in time (60 s view)"), ms);
        // Scroll the tracks up and down at the whole-session zoom.
        st.session_view = full;
        let mut ms = Vec::new();
        for i in 0..120 {
            caches.drag.scroll_y += if i < 60 { 30.0 } else { -30.0 };
            ms.push(frame(&ctx, &mut st, &mut caches));
        }
        report(&format!("{label}: scroll tracks"), ms);
        // Even track zoom from the headers.
        let mut ms = Vec::new();
        for i in 0..60 {
            let n = if i < 30 { 1.0 } else { -1.0 };
            for t in &mut st.session.tracks {
                t.height = crate::zoom::wheel_height(t.height, n);
            }
            ms.push(frame(&ctx, &mut st, &mut caches));
        }
        report(&format!("{label}: track height zoom"), ms);
        // Still frames.
        let mut ms = Vec::new();
        for _ in 0..60 {
            ms.push(frame(&ctx, &mut st, &mut caches));
        }
        report(&format!("{label}: idle"), ms);
    }
}

/// `AF2_BENCH_FILE=<session file>`: a real session (any format AF2 opens)
/// with its audio loaded, timed for import, drawing while zooming and
/// scrolling, and the audio engine block by block (the slowest blocks are
/// what a stutter is made of). `AF2_BENCH_PLUGINS=1` keeps VST3 plug-ins.
#[test]
#[ignore]
fn real_session_bench() {
    let Some(path) = std::env::var_os("AF2_BENCH_FILE").map(std::path::PathBuf::from) else {
        return;
    };
    let t = Instant::now();
    let imported = af27_interop::import(&path).expect("import");
    eprintln!("import: {:.2} s", t.elapsed().as_secs_f64());
    let mut st = State::new();
    st.mode = Mode::Multitrack;
    st.session = imported.session;
    if std::env::var_os("AF2_BENCH_PLUGINS").is_none() {
        for tr in &mut st.session.tracks {
            tr.chain
                .retain(|s| !af27_core::plugin::is_plugin(&s.effect));
            for c in &mut tr.clips {
                c.props
                    .chain
                    .retain(|s| !af27_core::plugin::is_plugin(&s.effect));
            }
        }
        st.session
            .master_chain
            .retain(|s| !af27_core::plugin::is_plugin(&s.effect));
    }
    let t = Instant::now();
    let media: Vec<(af27_core::MediaId, std::path::PathBuf)> = st
        .session
        .media
        .iter()
        .filter_map(|m| Some((m.id, m.path.clone()?)))
        .collect();
    let mut missing = 0;
    for (id, p) in media {
        match af27_io::decode_file(&p) {
            Ok(buf) => {
                let d = st.add_doc(
                    p.file_name().unwrap().to_string_lossy().to_string(),
                    Some(p.clone()),
                    buf,
                );
                st.media_docs.insert(id, d);
            }
            Err(_) => missing += 1,
        }
    }
    let decoded: usize = st
        .docs
        .iter()
        .map(|d| d.buf.frames() * d.buf.num_channels() * 4)
        .sum();
    eprintln!(
        "decoded audio held: {:.0} MB; process private {:.0} MB",
        decoded as f64 / 1048576.0,
        crate::memstat::private_mb()
    );
    let clips: usize = st.session.tracks.iter().map(|t| t.clips.len()).sum();
    let fx: usize = st.session.tracks.iter().map(|t| t.chain.len()).sum();
    eprintln!(
        "audio load: {:.2} s; {} tracks, {clips} clips, {fx} track effects, {} media ({missing} missing), {:.0} s long",
        t.elapsed().as_secs_f64(),
        st.session.tracks.len(),
        st.session.media.len(),
        st.session.duration()
    );
    st.session_view = View::fit(st.session.duration().max(30.0));

    // Drawing.
    let ctx = egui::Context::default();
    let mut caches = crate::ui::Caches::default();
    for _ in 0..5 {
        frame(&ctx, &mut st, &mut caches);
    }
    let lim = st.session_limits();
    let full = st.session_view;
    let anchor = full.start + full.len * 0.37;
    let mut ms = Vec::new();
    for i in 0..120 {
        st.session_view
            .zoom_at(if i < 60 { 0.8 } else { 1.25 }, anchor, &lim);
        ms.push(frame(&ctx, &mut st, &mut caches));
    }
    report("draw: wheel zoom", ms);
    st.session_view = View {
        start: 0.0,
        len: 60.0,
    };
    let mut ms = Vec::new();
    for _ in 0..120 {
        st.session_view.pan_px(25.0, st.session_px as f64, &lim);
        ms.push(frame(&ctx, &mut st, &mut caches));
    }
    report("draw: scroll in time (60 s view)", ms);
    st.session_view = full;
    let mut ms = Vec::new();
    for i in 0..120 {
        caches.drag.scroll_y += if i < 60 { 30.0 } else { -30.0 };
        ms.push(frame(&ctx, &mut st, &mut caches));
    }
    report("draw: scroll tracks", ms);
    let mut ms = Vec::new();
    for _ in 0..60 {
        ms.push(frame(&ctx, &mut st, &mut caches));
    }
    report("draw: idle", ms);
    eprintln!("after drawing: private {:.0} MB", crate::memstat::private_mb());

    // The audio engine: one block at a time from a third of the way in,
    // as the audio thread runs it.
    let rate = 48_000u32;
    let t = Instant::now();
    let program = af27_engine::mix_program(&st.session, &|m| st.media_buffer(m), rate);
    eprintln!(
        "mix build: {:.1} ms; private {:.0} MB",
        t.elapsed().as_secs_f64() * 1e3,
        crate::memstat::private_mb()
    );
    while af27_engine::stretch::pending() > 0 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!(
        "stretched clips ready: {:.0} MB held; private {:.0} MB",
        af27_engine::stretch::cache_bytes().0 as f64 / 1048576.0,
        crate::memstat::private_mb()
    );
    for _ in 0..5 {
        frame(&ctx, &mut st, &mut caches);
    }
    eprintln!("drawn with stretched clips: private {:.0} MB", crate::memstat::private_mb());
    let af27_engine::Program::Mix(mut mix) = program else {
        panic!("not a mix")
    };
    let start = st.session.duration() / 3.0;
    let dt = 1.0 / rate as f64;
    let block = 512usize;
    let budget = block as f64 / rate as f64 * 1e3;
    let t = Instant::now();
    mix.start(start);
    eprintln!("start: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    mix.prime(start);
    eprintln!(
        "prime (a mix arriving while playing): {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let mut out = vec![0.0f32; block * 2];
    let mut ms = Vec::new();
    let blocks = (20.0 * rate as f64 / block as f64) as usize;
    let mut at = start;
    for _ in 0..blocks {
        let t = Instant::now();
        mix.render(at, dt, block, &mut out, false);
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        at += block as f64 * dt;
    }
    let over = ms.iter().filter(|&&m| m > budget).count();
    report(
        &format!("audio: {block}-frame block (budget {budget:.1} ms)"),
        ms,
    );
    eprintln!("audio: {over} of {blocks} blocks over budget");
    eprintln!("after playing 20 s: private {:.0} MB", crate::memstat::private_mb());
}
