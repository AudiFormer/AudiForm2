// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Waveform editor: one file, every channel stacked, an overview bar,
//! a time ruler with markers, and an optional spectral display under the
//! waveform. Drag to select time, click to place the playhead.
//!
//! Wheel (see [`crate::zoom`]): over the waveform and the spectrum it
//! zooms time (around the pointer, between the pointer and the middle, or
//! the playhead, per the zoom preferences; or scrolls when wheel zoom is
//! off); over the ruler and the overview it always zooms toward the
//! pointer; Shift+wheel scrolls in time; Ctrl+wheel always zooms time;
//! Alt+wheel zooms the waveform's amplitude (display only, up to 16x).
//! Over the spectrum's frequency scale the wheel zooms the frequencies;
//! a right-click on the spectrum offers its zoom, scale and resolution.

use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Pos2, Rect, Rounding, Sense, Shape, Stroke};

use crate::i18n::tr;
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
    amp: f32,
) {
    let amp = amp.clamp(1.0, crate::zoom::MAX_AMP);
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
                let s = (data[i] * amp).clamp(-1.08, 1.08);
                pos2(time_to_x(v, rect, t), mid - s * half)
            })
            .collect();
        if th.glow > 0.0 {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(4.0_f32, ca(centre, (40.0 * th.glow) as u8)),
            ));
        }
        p.add(Shape::line(pts.clone(), Stroke::new(1.5_f32, c(centre))));
        if spp <= 1.0 / 6.0 {
            // Samples 6+ pixels apart: sample dots in the bright waveform colour, ringed in the
            // lane background so they stand out on any theme.
            let _ = peak;
            let dot = crate::theme::lerp(centre, [255, 255, 255], 0.35);
            for q in pts {
                p.circle_filled(q, 3.4, c(th.wave_background));
                p.circle_filled(q, 2.6, dot);
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
        let (lo, hi) = (lo * amp, hi * amp);
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
        st.theme.name.clone() + &format!("{:?}{:?}", st.theme.spectrum, st.spec),
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
    let size = st.spec.fft;
    let ny = rate / 2.0;
    let bins = size / 2;
    // Each row's place between bins, top row the highest frequency, on
    // the view's range and scale.
    let row_bins: Vec<(usize, f32)> = (0..rows_per_ch)
        .map(|y| {
            let pos = 1.0 - (y as f64 + 0.5) / rows_per_ch as f64;
            let f = st.spec.freq_at(pos, ny);
            let b = (f / ny * bins as f64).clamp(0.0, (bins - 1) as f64);
            (b.floor() as usize, b.fract() as f32)
        })
        .collect();
    let mut img = egui::ColorImage::new([cols, rows_per_ch * nch], Color32::BLACK);
    for ch in 0..nch {
        let spec = af27_dsp::fft::spectrogram(&d.buf.channels[ch], start, end, cols, size);
        for (x, col) in spec.iter().enumerate() {
            for (y, &(b, fr)) in row_bins.iter().enumerate() {
                let b1 = (b + 1).min(bins - 1);
                let db = col[b] + (col[b1] - col[b]) * fr;
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
            tr("Open or drop an audio file to start"),
            tr("Ctrl+O to open • WAV, FLAC, MP3, OGG, M4A, AIFF, MOGG, DTS WAV and more"),
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
    st.wave_px = body.width();
    let hover = resp.hover_pos();
    if let Some(hp) = hover {
        let input = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers));
        let areas = Areas {
            overview,
            ruler,
            body,
            wave: wave_area,
            spec: spec_area,
        };
        wheel(st, &areas, hp, input);
    }
    let mut moved_by_hand = false;
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
                        moved_by_hand = true;
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
                    moved_by_hand = true;
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
    if moved_by_hand {
        st.hand_moved_view();
    }
    let was_playing = st.is_playing();
    let on_overview = resp
        .interact_pointer_pos()
        .is_some_and(|pp| overview.contains(pp));
    if resp.clicked() && was_playing && !on_overview {
        st.play();
    }
    // Right-click on the spectrum: its view menu.
    if let Some(sa) = spec_area {
        spectrum_menu(ui, st, &resp, sa);
    }

    let pos = st.position();
    {
        // Whatever moved the view this frame, it stays inside the file.
        let d = st.doc_mut().unwrap();
        let dur = d.duration();
        d.view.clamp_to(dur);
    }
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
        1.0,
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
            d.amp,
        );
        let label = channel_name(nch, ch);
        p.text(
            lane.right_top() + vec2(-6.0, 4.0),
            Align2::RIGHT_TOP,
            label,
            FontId::proportional(10.0),
            c(th.text_dim),
        );
        // dB scale at the right edge (following the amplitude zoom).
        for db in [-6.0f32, -12.0, -24.0] {
            let g = 10f32.powf(db / 20.0) * d.amp * lane.height() * 0.46;
            if g > lane.height() * 0.5 {
                continue;
            }
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
        let rows = ((sa.height() / nch.min(8) as f32) as usize).clamp(64, 512);
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
        let ny = d.buf.rate as f64 / 2.0;
        let scale = Rect::from_min_max(pos2(sa.right() - SCALE_W, sa.top()), sa.max);
        p.rect_filled(scale, Rounding::ZERO, ca(th.panel, 150));
        for ch in 0..nch.min(8) {
            let lane = Rect::from_min_size(
                pos2(sa.left(), sa.top() + lane_h * ch as f32),
                vec2(sa.width(), lane_h),
            );
            for (f, label) in st.spec.ticks(ny, lane.height(), 14.0) {
                let y = lane.bottom() - st.spec.pos(f, ny) as f32 * lane.height();
                p.line_segment(
                    [pos2(lane.right() - 4.0, y), pos2(lane.right(), y)],
                    Stroke::new(1.0_f32, ca(th.text, 150)),
                );
                let y = y.clamp(lane.top() + 6.0, lane.bottom() - 6.0);
                p.text(
                    pos2(lane.right() - 6.0, y),
                    Align2::RIGHT_CENTER,
                    label,
                    FontId::proportional(9.0),
                    ca(th.text, 190),
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

/// Width of the spectrum's frequency scale at its right edge.
const SCALE_W: f32 = 44.0;

/// The Waveform editor's parts, for the wheel.
struct Areas {
    overview: Rect,
    ruler: Rect,
    body: Rect,
    wave: Rect,
    spec: Option<Rect>,
}

/// The mouse wheel over the Waveform editor (the rules are in the module
/// notes at the top).
fn wheel(
    st: &mut State,
    a: &Areas,
    hp: Pos2,
    (scroll, zoomd, m): (egui::Vec2, f32, egui::Modifiers),
) {
    let prefs = st.zoom;
    let pos = st.position();
    let min = st.min_view_len();
    let ctrl = if zoomd != 1.0 {
        crate::zoom::ctrl_points(zoomd)
    } else {
        0.0
    };
    let on_top = a.overview.contains(hp) || a.ruler.contains(hp);
    let on_scale = a
        .spec
        .is_some_and(|sa| sa.contains(hp) && hp.x >= sa.right() - SCALE_W);
    let Some(d) = st.doc_mut() else { return };
    let dur = d.duration();
    // The time under the pointer: on the overview, its place in the file.
    let pointer_t = if a.overview.contains(hp) {
        ((hp.x - a.overview.left()) / a.overview.width()) as f64 * dur
    } else {
        x_to_time(&d.view, a.body, hp.x)
    };
    let anchor = if on_top {
        pointer_t
    } else {
        crate::zoom::anchor_time(prefs.wheel_anchor, pointer_t, &d.view, pos)
    };
    let mut moved = false;
    if ctrl != 0.0 {
        d.view.zoom_min(prefs.wheel_factor(ctrl), anchor, dur, min);
        moved = true;
    } else if scroll.y != 0.0 {
        if on_scale && !m.alt {
            // Frequency zoom: the frequency under the pointer stays put.
            let sa = a.spec.unwrap_or(a.body);
            let nch = d.buf.num_channels().clamp(1, 8);
            let lane_h = sa.height() / nch as f32;
            let k = (((hp.y - sa.top()) / lane_h).floor() as usize).min(nch - 1);
            let bottom = sa.top() + lane_h * (k + 1) as f32;
            let frac = ((bottom - hp.y) / lane_h).clamp(0.0, 1.0) as f64;
            let ny = d.buf.rate as f64 / 2.0;
            let f = st.spec.freq_at(frac, ny);
            st.spec.zoom(prefs.wheel_factor(scroll.y), f, ny);
            return;
        } else if m.alt {
            if a.wave.contains(hp) {
                let f = 1.0 / prefs.wheel_factor(scroll.y) as f32;
                d.amp = (d.amp * f).clamp(1.0, crate::zoom::MAX_AMP);
            }
        } else if on_top || prefs.wheel_zooms_waveform {
            d.view
                .zoom_min(prefs.wheel_factor(scroll.y), anchor, dur, min);
            moved = true;
        } else {
            d.view.start -= scroll.y as f64 / a.body.width() as f64 * d.view.len;
            d.view.clamp_to(dur);
            moved = true;
        }
    }
    if scroll.x != 0.0 && !m.alt {
        // Shift+wheel (egui turns it sideways) or a sideways swipe.
        d.view.start -= scroll.x as f64 / a.body.width() as f64 * d.view.len;
        d.view.clamp_to(dur);
        moved = true;
    }
    if moved {
        st.hand_moved_view();
    }
}

/// The spectrum's right-click menu: frequency zoom, scale, resolution.
fn spectrum_menu(ui: &egui::Ui, st: &mut State, resp: &egui::Response, sa: Rect) {
    let id = egui::Id::new("af27-spectrum-menu");
    if resp.secondary_clicked() {
        let on = resp.interact_pointer_pos().is_some_and(|p| sa.contains(p));
        ui.data_mut(|d| d.insert_temp(id, on));
    }
    let ny = st.doc().map_or(24_000.0, |d| d.buf.rate as f64 / 2.0);
    resp.context_menu(|ui| {
        if !ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
            ui.close_menu();
            return;
        }
        let s = &mut st.spec;
        if ui.button(tr("Zoom In (Frequency)")).clicked() {
            s.zoom_step(true, ny);
            ui.close_menu();
        }
        if ui.button(tr("Zoom Out (Frequency)")).clicked() {
            s.zoom_step(false, ny);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                s.range.is_some(),
                egui::Button::new(tr("Zoom Reset (Frequency)")),
            )
            .clicked()
        {
            s.reset_zoom();
            ui.close_menu();
        }
        ui.separator();
        if ui.radio(!s.notes, tr("Hertz")).clicked() {
            s.notes = false;
            ui.close_menu();
        }
        if ui.radio(s.notes, tr("Notes")).clicked() {
            s.notes = true;
            ui.close_menu();
        }
        ui.separator();
        let full_log = s.log_step == crate::zoom::LOG_STEPS;
        let full_lin = s.log_step == 0;
        if ui
            .add_enabled(!full_log, egui::Button::new(tr("More Logarithmic")))
            .clicked()
        {
            s.more_log();
        }
        if ui
            .add_enabled(!full_lin, egui::Button::new(tr("More Linear")))
            .clicked()
        {
            s.more_linear();
        }
        if ui
            .add_enabled(!full_log, egui::Button::new(tr("Full Logarithmic")))
            .clicked()
        {
            s.log_step = crate::zoom::LOG_STEPS;
            ui.close_menu();
        }
        if ui
            .add_enabled(!full_lin, egui::Button::new(tr("Full Linear")))
            .clicked()
        {
            s.log_step = 0;
            ui.close_menu();
        }
        ui.separator();
        if ui
            .add_enabled(
                s.fft < crate::zoom::FFT_MAX,
                egui::Button::new(tr("Increase Spectral Resolution")),
            )
            .clicked()
        {
            s.finer();
        }
        if ui
            .add_enabled(
                s.fft > crate::zoom::FFT_MIN,
                egui::Button::new(tr("Decrease Spectral Resolution")),
            )
            .clicked()
        {
            s.coarser();
        }
        ui.label(
            egui::RichText::new(tr!("Analysis frame: {0} samples", s.fft))
                .small()
                .weak(),
        );
    });
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

/// The playhead: the HUD's laser. The motion and music state come from
/// the shared [`crate::hud::Fx`] the app keeps in egui's memory each frame.
pub fn playhead(p: &egui::Painter, x: f32, top: f32, bottom: f32, th: &Theme) {
    let fx: crate::hud::Fx = p
        .ctx()
        .data(|d| d.get_temp(egui::Id::new("af27-fx")))
        .unwrap_or_default();
    crate::hud::laser(p, x, top, bottom, th, &fx);
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
