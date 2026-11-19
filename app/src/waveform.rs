// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! The Waveform editor: one file, every channel stacked, an overview bar,
//! a time ruler with markers, and an optional spectral display under the
//! waveform. Drag to select time, click to place the playhead, mouse
//! wheel zooms at the pointer, Shift+wheel scrolls.

use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Pos2, Rect, Rounding, Sense, Shape, Stroke};

use crate::state::{State, View};
use crate::theme::{c, ca, Theme};
use crate::ui::Caches;
use af27_dsp::Peaks;

#[derive(Debug, Clone, Copy)]
pub enum WaveDrag {
    Select { anchor: f64 },
    Overview,
}

pub fn time_to_x(v: &View, rect: Rect, t: f64) -> f32 {
    rect.left() + ((t - v.start) / v.len) as f32 * rect.width()
}

pub fn x_to_time(v: &View, rect: Rect, x: f32) -> f64 {
    v.start + ((x - rect.left()) / rect.width()) as f64 * v.len
}

/// A "nice" ruler step for `len` seconds across `px` pixels.
fn ruler_step(len: f64, px: f32) -> f64 {
    let target = len / (px as f64 / 90.0).max(1.0);
    let steps = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0,
        60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
    ];
    steps
        .iter()
        .copied()
        .find(|s| *s >= target)
        .unwrap_or(3600.0)
}

fn ruler_label(t: f64, step: f64) -> String {
    if step < 1.0 {
        let m = (t / 60.0).floor();
        let s = t - m * 60.0;
        let dec = if step < 0.01 {
            3
        } else if step < 0.1 {
            2
        } else {
            1
        };
        format!("{}:{:0w$.dec$}", m as u64, s, w = dec + 3, dec = dec)
    } else {
        let t = t.round() as u64;
        if t >= 3600 {
            format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
        } else {
            format!("{}:{:02}", t / 60, t % 60)
        }
    }
}

pub fn draw_ruler(
    p: &egui::Painter,
    rect: Rect,
    v: &View,
    th: &Theme,
    markers: &[af27_core::Marker],
) {
    p.rect_filled(rect, Rounding::ZERO, c(th.panel));
    let step = ruler_step(v.len, rect.width());
    let first = (v.start / step).floor() as i64;
    let last = ((v.start + v.len) / step).ceil() as i64;
    for i in first..=last {
        let t = i as f64 * step;
        let x = time_to_x(v, rect, t);
        if x < rect.left() - 1.0 || x > rect.right() {
            continue;
        }
        p.line_segment(
            [pos2(x, rect.bottom() - 7.0), pos2(x, rect.bottom())],
            Stroke::new(1.0_f32, c(th.text_dim)),
        );
        let xm = time_to_x(v, rect, t + step / 2.0);
        p.line_segment(
            [pos2(xm, rect.bottom() - 3.0), pos2(xm, rect.bottom())],
            Stroke::new(1.0_f32, c(th.border)),
        );
        p.text(
            pos2(x + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            ruler_label(t, step),
            FontId::proportional(10.0),
            c(th.text_dim),
        );
    }
    for m in markers {
        let x = time_to_x(v, rect, m.time);
        if x >= rect.left() && x <= rect.right() {
            let tri = vec![
                pos2(x, rect.bottom()),
                pos2(x - 5.0, rect.bottom() - 8.0),
                pos2(x + 5.0, rect.bottom() - 8.0),
            ];
            p.add(Shape::convex_polygon(tri, c(th.marker), Stroke::NONE));
        }
    }
}

/// Draws one channel's waveform with a centre-to-peak gradient.
#[allow(clippy::too_many_arguments)]
pub fn draw_channel(
    p: &egui::Painter,
    rect: Rect,
    data: &[f32],
    peaks: &Peaks,
    ch: usize,
    rate: u32,
    v: &View,
    th: &Theme,
    tint: Option<[u8; 3]>,
) {
    let mid = rect.center().y;
    let half = rect.height() * 0.46;
    let w = rect.width().max(1.0) as usize;
    let spp = v.len * rate as f64 / w as f64;
    let (centre, peak) = match tint {
        Some(t) => (
            t,
            crate::theme::lerp(t, [255, 255, 255], 0.35).to_array()[..3]
                .try_into()
                .unwrap(),
        ),
        None => (th.wave_centre, th.wave_peak),
    };
    p.line_segment(
        [pos2(rect.left(), mid), pos2(rect.right(), mid)],
        Stroke::new(1.0_f32, ca(centre, 60)),
    );
    if spp <= 1.5 {
        // Zoomed in: a line through the samples, with dots when very close.
        let a = (v.start * rate as f64).floor().max(0.0) as usize;
        let b = (((v.start + v.len) * rate as f64).ceil() as usize + 1).min(data.len());
        let pts: Vec<Pos2> = (a..b)
            .map(|i| {
                let t = i as f64 / rate as f64;
                pos2(time_to_x(v, rect, t), mid - data[i].clamp(-1.5, 1.5) * half)
            })
            .collect();
        if th.glow > 0.0 {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(4.0_f32, ca(centre, (40.0 * th.glow) as u8)),
            ));
        }
        p.add(Shape::line(pts.clone(), Stroke::new(1.5_f32, c(centre))));
        if spp < 0.12 {
            for q in pts {
                p.circle_filled(q, 2.5, c(peak));
            }
        }
        return;
    }
    let mut mesh = Mesh::default();
    let mut top_line = Vec::with_capacity(w);
    let mut bot_line = Vec::with_capacity(w);
    for x in 0..w {
        let t0 = v.start + v.len * x as f64 / w as f64;
        let t1 = v.start + v.len * (x + 1) as f64 / w as f64;
        let s0 = (t0 * rate as f64).max(0.0) as usize;
        let s1 = ((t1 * rate as f64) as usize).max(s0 + 1);
        if s0 >= data.len() {
            break;
        }
        let (lo, hi) = peaks.range(data, ch, s0, s1);
        let xf = rect.left() + x as f32;
        let yt = mid - hi.clamp(-1.0, 1.0) * half;
        let yb = mid - lo.clamp(-1.0, 1.0) * half;
        let amp = hi.abs().max(lo.abs()).min(1.0);
        let edge = crate::theme::lerp(centre, peak, amp.sqrt());
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(xf, yt.min(mid - 0.5)), edge);
        mesh.colored_vertex(pos2(xf + 1.0, yt.min(mid - 0.5)), edge);
        mesh.colored_vertex(pos2(xf + 1.0, mid), c(centre));
        mesh.colored_vertex(pos2(xf, mid), c(centre));
        mesh.colored_vertex(pos2(xf, yb.max(mid + 0.5)), edge);
        mesh.colored_vertex(pos2(xf + 1.0, yb.max(mid + 0.5)), edge);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
        mesh.add_triangle(i + 3, i + 2, i + 5);
        mesh.add_triangle(i + 3, i + 5, i + 4);
        top_line.push(pos2(xf, yt));
        bot_line.push(pos2(xf, yb));
    }
    if th.glow > 0.0 && top_line.len() > 1 {
        let s = Stroke::new(3.0_f32, ca(peak, (45.0 * th.glow) as u8));
        p.add(Shape::line(top_line, s));
        p.add(Shape::line(bot_line, s));
    }
    p.add(Shape::mesh(mesh));
}

fn spectrogram_texture(
    ui: &egui::Ui,
    st: &State,
    caches: &mut Caches,
    cols: usize,
    rows_per_ch: usize,
) -> Option<egui::TextureHandle> {
    let d = st.doc()?;
    let key = (
        d.version,
        (d.view.start * 1000.0) as i64,
        (d.view.len * 100_000.0) as i64,
        cols,
        rows_per_ch,
        st.theme.name.clone() + &format!("{:?}", st.theme.spectrum),
    );
    if let Some((tex, k)) = &caches.spectro {
        if *k == key {
            return Some(tex.clone());
        }
    }
    let rate = d.buf.rate as f64;
    let start = (d.view.start * rate) as usize;
    let end = ((d.view.start + d.view.len) * rate) as usize;
    let nch = d.buf.num_channels().min(8);
    let size = 2048;
    let mut img = egui::ColorImage::new([cols, rows_per_ch * nch], Color32::BLACK);
    for ch in 0..nch {
        let spec = af27_dsp::fft::spectrogram(&d.buf.channels[ch], start, end, cols, size);
        let bins = size / 2;
        for (x, col) in spec.iter().enumerate() {
            for y in 0..rows_per_ch {
                // Top row is the highest frequency; a gentle curve gives
                // the low end more room.
                let f = 1.0 - (y as f32 + 0.5) / rows_per_ch as f32;
                let bin = ((f.powf(1.6)) * (bins - 1) as f32) as usize;
                let db = col[bin.min(bins - 1)];
                let level = ((db + 110.0) / 100.0).clamp(0.0, 1.0);
                let [r, g, b] = st.theme.spectrum_color(level);
                img[(x, ch * rows_per_ch + y)] = Color32::from_rgb(r, g, b);
            }
        }
    }
    let tex = ui
        .ctx()
        .load_texture("spectrogram", img, egui::TextureOptions::LINEAR);
    caches.spectro = Some((tex.clone(), key));
    Some(tex)
}

pub fn editor(ui: &mut egui::Ui, st: &mut State, caches: &mut Caches) {
    let th = st.theme.clone();
    if st.doc().is_none() {
        empty_state(
            ui,
            &th,
            "Open or drop an audio file to start",
            "Ctrl+O to open • WAV, FLAC, MP3, OGG, M4A, AIFF, MOGG, DTS WAV and more",
        );
        return;
    }
    // File tabs for every open document.
    ui.horizontal_wrapped(|ui| {
        let docs: Vec<_> = st
            .docs
            .iter()
            .map(|d| (d.id, d.name.clone(), d.dirty))
            .collect();
        for (id, name, dirty) in docs {
            let on = st.current == Some(id);
            let label = format!("{}{}", name, if dirty { " •" } else { "" });
            if ui.selectable_label(on, label).clicked() {
                st.current = Some(id);
            }
        }
    });
    let full = ui.available_rect_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let overview = Rect::from_min_size(rect.min, vec2(rect.width(), 22.0));
    let ruler = Rect::from_min_size(
        pos2(rect.left(), overview.bottom() + 2.0),
        vec2(rect.width(), 20.0),
    );
    let body = Rect::from_min_max(pos2(rect.left(), ruler.bottom()), rect.max);
    let spectral = st.doc().is_some_and(|d| d.spectral);
    let (wave_area, spec_area) = if spectral {
        let split = body.top() + body.height() * 0.5;
        (
            Rect::from_min_max(body.min, pos2(body.right(), split - 2.0)),
            Some(Rect::from_min_max(pos2(body.left(), split + 2.0), body.max)),
        )
    } else {
        (body, None)
    };

    // Interaction first so drawing shows this frame's state.
    let hover = resp.hover_pos();
    if let Some(hp) = hover {
        let (scroll, zoomd, shift) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
        let d = st.doc_mut().unwrap();
        let dur = d.duration();
        if zoomd != 1.0 {
            let t = x_to_time(&d.view, body, hp.x);
            d.view.zoom(1.0 / zoomd as f64, t, dur);
        } else if scroll.y != 0.0 && !shift {
            let t = x_to_time(&d.view, body, hp.x);
            d.view.zoom((-scroll.y as f64 * 0.004).exp(), t, dur);
        } else if scroll.y != 0.0 || scroll.x != 0.0 {
            let dx = if shift { scroll.y } else { scroll.x };
            d.view.start = (d.view.start - dx as f64 / body.width() as f64 * d.view.len).max(0.0);
        }
    }
    {
        let d = st.doc_mut().unwrap();
        let dur = d.duration();
        if resp.drag_started() {
            if let Some(pp) = resp.interact_pointer_pos() {
                if overview.contains(pp) {
                    caches.wave_drag = Some(WaveDrag::Overview);
                } else {
                    let t = x_to_time(&d.view, body, pp.x).clamp(0.0, dur);
                    // Grab the far edge when starting on an existing edge.
                    let anchor = match d.sel {
                        Some((a, b)) => {
                            let near = |e: f64| (time_to_x(&d.view, body, e) - pp.x).abs() < 6.0;
                            if near(a) {
                                b
                            } else if near(b) {
                                a
                            } else {
                                t
                            }
                        }
                        None => t,
                    };
                    caches.wave_drag = Some(WaveDrag::Select { anchor });
                }
            }
        }
        if let (Some(drag), Some(pp)) = (caches.wave_drag, resp.interact_pointer_pos()) {
            if resp.dragged() {
                match drag {
                    WaveDrag::Overview => {
                        let t = ((pp.x - overview.left()) / overview.width()) as f64 * dur;
                        d.view.start = (t - d.view.len / 2.0).max(0.0);
                    }
                    WaveDrag::Select { anchor } => {
                        let t = x_to_time(&d.view, body, pp.x).clamp(0.0, dur);
                        d.sel = Some((anchor.min(t), anchor.max(t)));
                        d.cursor = anchor.min(t);
                        // Scroll while dragging past an edge.
                        if pp.x > body.right() - 4.0 {
                            d.view.start += d.view.len * 0.02;
                        } else if pp.x < body.left() + 4.0 {
                            d.view.start = (d.view.start - d.view.len * 0.02).max(0.0);
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            caches.wave_drag = None;
        }
        if resp.clicked() {
            if let Some(pp) = resp.interact_pointer_pos() {
                if overview.contains(pp) {
                    let t = ((pp.x - overview.left()) / overview.width()) as f64 * dur;
                    d.view.start = (t - d.view.len / 2.0).max(0.0);
                } else {
                    d.cursor = x_to_time(&d.view, body, pp.x).clamp(0.0, dur);
                    d.sel = None;
                }
            }
        }
        if resp.double_clicked() {
            d.sel = Some((0.0, dur));
        }
    }
    let was_playing = st.is_playing();
    if resp.clicked() && was_playing {
        st.play();
    }

    let pos = st.position();
    let d = st.doc().unwrap();
    let v = d.view;
    let dur = d.duration();
    let nch = d.buf.num_channels().max(1);

    // Overview: whole file in miniature, with the visible window.
    p.rect_filled(overview, Rounding::same(th.rounding), c(th.wave_background));
    let ov = View {
        start: 0.0,
        len: dur.max(1e-6),
    };
    draw_channel(
        &p,
        overview.shrink(2.0),
        &d.buf.channels[0],
        &d.peaks,
        0,
        d.buf.rate,
        &ov,
        &th,
        None,
    );
    let vx0 = time_to_x(&ov, overview, v.start);
    let vx1 = time_to_x(&ov, overview, v.start + v.len);
    let win = Rect::from_min_max(
        pos2(vx0.max(overview.left()), overview.top()),
        pos2(vx1.min(overview.right()), overview.bottom()),
    );
    p.rect_filled(win, Rounding::same(3.0), ca(th.accent, 30));
    p.rect_stroke(win, Rounding::same(3.0), Stroke::new(1.0_f32, c(th.accent)));

    draw_ruler(&p, ruler, &v, &th, &d.markers);

    // Waveform lanes.
    p.rect_filled(wave_area, Rounding::ZERO, c(th.wave_background));
    let step = ruler_step(v.len, wave_area.width());
    let first = (v.start / step).floor() as i64;
    for i in first..=((v.start + v.len) / step).ceil() as i64 {
        let x = time_to_x(&v, wave_area, i as f64 * step);
        p.line_segment(
            [pos2(x, body.top()), pos2(x, body.bottom())],
            Stroke::new(1.0_f32, c(th.grid)),
        );
    }
    let lane_h = wave_area.height() / nch as f32;
    for ch in 0..nch {
        let lane = Rect::from_min_size(
            pos2(wave_area.left(), wave_area.top() + lane_h * ch as f32),
            vec2(wave_area.width(), lane_h),
        );
        if ch > 0 {
            p.line_segment(
                [lane.left_top(), lane.right_top()],
                Stroke::new(1.0_f32, c(th.border)),
            );
        }
        draw_channel(
            &p,
            lane,
            &d.buf.channels[ch],
            &d.peaks,
            ch,
            d.buf.rate,
            &v,
            &th,
            None,
        );
        let label = channel_name(nch, ch);
        p.text(
            lane.right_top() + vec2(-6.0, 4.0),
            Align2::RIGHT_TOP,
            label,
            FontId::proportional(10.0),
            c(th.text_dim),
        );
        // dB scale at the right edge.
        for db in [-6.0f32, -12.0, -24.0] {
            let g = 10f32.powf(db / 20.0) * lane.height() * 0.46;
            for y in [lane.center().y - g, lane.center().y + g] {
                p.line_segment(
                    [pos2(lane.right() - 6.0, y), pos2(lane.right(), y)],
                    Stroke::new(1.0_f32, c(th.border)),
                );
            }
        }
    }

    // Spectral display.
    if let Some(sa) = spec_area {
        let cols = ((sa.width() / 2.0) as usize).clamp(32, 1024);
        let rows = 192;
        if let Some(tex) = spectrogram_texture(ui, st, caches, cols, rows) {
            p.image(
                tex.id(),
                sa,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let d = st.doc().unwrap();
        let lane_h = sa.height() / nch.min(8) as f32;
        let ny = d.buf.rate as f32 / 2.0;
        for ch in 0..nch.min(8) {
            let lane = Rect::from_min_size(
                pos2(sa.left(), sa.top() + lane_h * ch as f32),
                vec2(sa.width(), lane_h),
            );
            for khz in [1.0f32, 5.0, 10.0, 15.0, 20.0] {
                let f = khz * 1000.0 / ny;
                if f >= 1.0 {
                    continue;
                }
                let y = lane.bottom() - f.powf(1.0 / 1.6) * lane.height();
                p.text(
                    pos2(lane.right() - 4.0, y),
                    Align2::RIGHT_CENTER,
                    format!("{khz}k"),
                    FontId::proportional(9.0),
                    ca(th.text, 150),
                );
            }
            if ch > 0 {
                p.line_segment(
                    [lane.left_top(), lane.right_top()],
                    Stroke::new(1.0_f32, c(th.background)),
                );
            }
        }
    }

    let d = st.doc().unwrap();
    // Selection wash.
    if let Some((a, b)) = d.sel {
        let x0 = time_to_x(&v, body, a).max(body.left());
        let x1 = time_to_x(&v, body, b).min(body.right());
        if x1 > x0 {
            let r = Rect::from_min_max(pos2(x0, body.top()), pos2(x1, body.bottom()));
            p.rect_filled(r, Rounding::ZERO, ca(th.selection, th.selection_alpha));
            for x in [x0, x1] {
                p.line_segment(
                    [pos2(x, body.top()), pos2(x, body.bottom())],
                    Stroke::new(1.0_f32, ca(th.selection, 160)),
                );
            }
        }
    }
    // Markers.
    for m in &d.markers {
        let x = time_to_x(&v, body, m.time);
        if x >= body.left() && x <= body.right() {
            dashed_v(&p, x, body.top(), body.bottom(), c(th.marker));
            p.text(
                pos2(x + 3.0, body.top() + 2.0),
                Align2::LEFT_TOP,
                &m.name,
                FontId::proportional(10.0),
                c(th.marker),
            );
        }
    }
    // Playhead.
    let x = time_to_x(&v, body, pos);
    if x >= body.left() && x <= body.right() {
        playhead(&p, x, ruler.top(), body.bottom(), &th);
    }
    if let Some(hp) = hover {
        if body.contains(hp) {
            let t = x_to_time(&v, body, hp.x);
            p.text(
                hp + vec2(10.0, -14.0),
                Align2::LEFT_BOTTOM,
                af27_core::time::format_time(t),
                FontId::monospace(10.0),
                ca(th.text, 200),
            );
        }
    }
}

pub fn channel_name(n: usize, ch: usize) -> &'static str {
    const STEREO: [&str; 2] = ["L", "R"];
    const SURROUND: [&str; 8] = ["L", "R", "C", "LFE", "Ls", "Rs", "Lb", "Rb"];
    match n {
        1 => "M",
        2 => STEREO[ch],
        _ => SURROUND.get(ch).copied().unwrap_or("·"),
    }
}

pub fn playhead(p: &egui::Painter, x: f32, top: f32, bottom: f32, th: &Theme) {
    if th.glow > 0.0 {
        p.line_segment(
            [pos2(x, top), pos2(x, bottom)],
            Stroke::new(5.0_f32, ca(th.playhead, (50.0 * th.glow) as u8)),
        );
    }
    p.line_segment(
        [pos2(x, top), pos2(x, bottom)],
        Stroke::new(1.5_f32, c(th.playhead)),
    );
    let tri = vec![pos2(x - 6.0, top), pos2(x + 6.0, top), pos2(x, top + 8.0)];
    p.add(Shape::convex_polygon(tri, c(th.playhead), Stroke::NONE));
}

pub fn dashed_v(p: &egui::Painter, x: f32, top: f32, bottom: f32, col: Color32) {
    let mut y = top;
    while y < bottom {
        p.line_segment(
            [pos2(x, y), pos2(x, (y + 5.0).min(bottom))],
            Stroke::new(1.0_f32, col),
        );
        y += 9.0;
    }
}

pub fn empty_state(ui: &mut egui::Ui, th: &Theme, title: &str, hint: &str) {
    let rect = ui.available_rect_before_wrap();
    let p = ui.painter_at(rect);
    p.rect_filled(rect, Rounding::ZERO, c(th.wave_background));
    // A faint decorative wave.
    let mid = rect.center().y;
    let pts: Vec<Pos2> = (0..=200)
        .map(|i| {
            let f = i as f32 / 200.0;
            let env = (f * std::f32::consts::PI).sin();
            pos2(
                rect.left() + f * rect.width(),
                mid + (f * 60.0).sin() * env * 40.0 * (f * 7.0).cos(),
            )
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(2.0_f32, ca(th.accent, 50))));
    p.text(
        rect.center() - vec2(0.0, 70.0),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(20.0),
        c(th.text),
    );
    p.text(
        rect.center() - vec2(0.0, 44.0),
        Align2::CENTER_CENTER,
        hint,
        FontId::proportional(12.0),
        c(th.text_dim),
    );
    ui.allocate_rect(rect, Sense::hover());
}
