// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Waveform editor: one file, every channel stacked, an overview bar,
//! a time ruler with markers, and an optional spectral display under the
//! waveform. Drag to select time (or drag a selection edge), click to
//! place the playhead (not while playing: then only the ruler moves it).
//! The ruler, the start marker and middle-button panning are in
//! [`crate::ruler`].
//!
//! Wheel (the full table is [`crate::zoom::wheel_action`]): over the
//! waveform and the spectrum it zooms time (around the pointer or the
//! playhead, per the zoom preferences; or scrolls when wheel zoom is off);
//! over the ruler and the overview it always zooms at the pointer;
//! Shift+wheel scrolls in time (over the ruler it zooms); Ctrl+wheel
//! always zooms time; Alt+wheel zooms the waveform's amplitude (display
//! only, twice per notch, up to 16x). Over the spectrum's frequency scale
//! the wheel zooms the frequencies at the pointer, and dragging the scale
//! slides through the frequencies when zoomed in; a right-click on the
//! spectrum offers its zoom, scale and resolution.

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
        // The glow only once samples are spread out (dense lines cost a
        // lot to draw and the glow would not show between them).
        if th.glow > 0.0 && spp <= 0.25 {
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
    // One strip of triangles for the whole channel: per pixel column a
    // vertex at the top, the centre line and the bottom (and, with glow,
    // a faded one just outside each edge), joined to the next column.
    // Far fewer vertices than a quad per column plus stroked outlines, so
    // dozens of tracks stay quick to zoom and scroll.
    // (Too thin to see on short lanes, where it is left out.)
    let glow = th.glow > 0.0 && half >= 12.0;
    let per = if glow { 5 } else { 3 };
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(w * per + per);
    mesh.indices.reserve(w * (per - 1) * 6);
    let clear = ca(peak, 0);
    let glow_w = th.glow.clamp(0.0, 1.5);
    let centre_c = c(centre);
    let mut cols = 0u32;
    let mut push = |mesh: &mut Mesh, xf: f32, yt: f32, yb: f32, edge: Color32| {
        let i = mesh.vertices.len() as u32;
        let yt = yt.min(mid - 0.5);
        let yb = yb.max(mid + 0.5);
        if glow {
            // The edge fades out over a few pixels: the glow.
            mesh.colored_vertex(pos2(xf, yt - 1.0 - 2.0 * glow_w), clear);
        }
        mesh.colored_vertex(pos2(xf, yt), edge);
        mesh.colored_vertex(pos2(xf, mid), centre_c);
        mesh.colored_vertex(pos2(xf, yb), edge);
        if glow {
            mesh.colored_vertex(pos2(xf, yb + 1.0 + 2.0 * glow_w), clear);
        }
        if cols > 0 {
            let prev = i - per as u32;
            for k in 0..(per as u32 - 1) {
                let (a, b) = (prev + k, prev + k + 1);
                let (c2, d) = (i + k, i + k + 1);
                mesh.add_triangle(a, c2, b);
                mesh.add_triangle(b, c2, d);
            }
        }
        cols += 1;
    };
    let mut last: Option<(f32, f32, f32, Color32)> = None;
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
        let level = hi.abs().max(lo.abs()).min(1.0);
        let edge = crate::theme::lerp(centre, peak, level.sqrt());
        push(&mut mesh, xf, yt, yb, edge);
        last = Some((xf, yt, yb, edge));
    }
    // Close the last pixel column.
    if let Some((xf, yt, yb, edge)) = last {
        push(&mut mesh, xf + 1.0, yt, yb, edge);
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
    // The document is part of the key: another file at the same version
    // and view must not show the drawing of one just closed.
    let key = (
        d.id,
        d.version,
        (d.view.start * 1000.0) as i64,
        (d.view.len * 100_000.0) as i64,
        cols,
        rows_per_ch,
        st.theme.name.clone()
            + &format!("{:?}{:?}{:?}", st.theme.spectrum, st.spec, st.spec_colors),
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
    let lut = crate::spectral_select::color_lut(st);
    let mut img = egui::ColorImage::new([cols, rows_per_ch * nch], Color32::BLACK);
    for ch in 0..nch {
        let spec = af27_dsp::fft::spectrogram(&d.buf.channels[ch], start, end, cols, size);
        for (x, col) in spec.iter().enumerate() {
            for (y, &(b, fr)) in row_bins.iter().enumerate() {
                let b1 = (b + 1).min(bins - 1);
                let db = col[b] + (col[b1] - col[b]) * fr;
                let level = ((db + 110.0) / 100.0).clamp(0.0, 1.0);
                let [r, g, b] = lut[(level * 255.0).round() as usize];
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
        let split = body.top() + body.height() * st.spec_split.clamp(0.1, 0.9);
        (
            Rect::from_min_max(body.min, pos2(body.right(), split - 2.0)),
            Some(Rect::from_min_max(pos2(body.left(), split + 2.0), body.max)),
        )
    } else {
        (body, None)
    };

    // The ruler is its own widget on top: it moves the playhead.
    let rresp = ui.interact(
        ruler,
        ui.id().with("af27-wave-ruler"),
        Sense::click_and_drag(),
    );
    // The spectral tools take the spectrum's pointer while chosen; the
    // divider sits between waveform and spectrum.
    let spec_resp = spec_area.and_then(|sa| crate::spectral_select::overlay(ui, st, sa));
    if spec_area.is_some() {
        crate::spectral_select::divider(ui, st, body, (wave_area.bottom() + 2.0).round());
    }

    // Interaction first so drawing shows this frame's state.
    st.wave_px = body.width();
    let hover = resp
        .hover_pos()
        .or(rresp.hover_pos())
        .or_else(|| spec_resp.as_ref().and_then(|r| r.hover_pos()));
    if let Some(hp) = hover {
        let input = ui.input(|i| {
            crate::zoom::WheelInput::read(
                i.raw_scroll_delta.into(),
                i.zoom_delta(),
                i.modifiers.command || i.modifiers.ctrl,
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        if let Some(input) = input {
            let areas = Areas {
                overview,
                ruler,
                body,
                wave: wave_area,
                spec: spec_area,
            };
            wheel(st, &areas, hp, input);
        }
    }
    // Dragging the frequency scale slides through the frequencies.
    if let Some(sa) = spec_area {
        freq_scale_drag(ui, st, sa);
    }
    let mut moved_by_hand = false;
    let playing = st.transport_playing();
    let snap_marks: Vec<f64> = st
        .doc()
        .map(|d| d.markers.iter().map(|m| m.time).collect())
        .unwrap_or_default();
    let ghost = {
        let (v, dur) = st
            .doc()
            .map_or((View::fit(1.0), 0.0), |d| (d.view, d.duration()));
        crate::ruler::input(ui, st, &rresp, body, &v, dur, &snap_marks, false)
    };
    // Middle-button drag anywhere pans in time.
    if let Some(dv) = crate::ruler::pan_delta(ui, &resp).or(crate::ruler::pan_delta(ui, &rresp)) {
        let lim = st.doc_limits();
        let d = st.doc_mut().unwrap();
        d.view.pan_px(-dv.x as f64, body.width() as f64, &lim);
        moved_by_hand = true;
    }
    let snap_view = st.doc().map(|d| d.view).unwrap_or(View::fit(1.0));
    let snapped = |st: &State, t: f64| crate::ruler::snap(st, t, &snap_view, body, &snap_marks);
    let drag_t = resp
        .interact_pointer_pos()
        .map(|pp| snapped(st, x_to_time(&snap_view, body, pp.x)));
    let on_edge = resp
        .interact_pointer_pos()
        .or(resp.hover_pos())
        .filter(|pp| body.contains(*pp))
        .and_then(|pp| crate::ruler::edge_at(st.doc().and_then(|d| d.sel), &snap_view, body, pp.x));
    if on_edge.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    {
        let d = st.doc_mut().unwrap();
        let dur = d.duration();
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(pp) = resp.interact_pointer_pos() {
                if overview.contains(pp) {
                    caches.wave_drag = Some(WaveDrag::Overview);
                } else {
                    let t = drag_t.unwrap_or(0.0).clamp(0.0, dur);
                    // Grab the far edge when starting on an existing edge.
                    let anchor = on_edge.map_or(t, |(_, other)| other);
                    caches.wave_drag = Some(WaveDrag::Select { anchor });
                }
            }
        }
        if let (Some(drag), Some(pp)) = (caches.wave_drag, resp.interact_pointer_pos()) {
            if resp.dragged_by(egui::PointerButton::Primary) {
                match drag {
                    WaveDrag::Overview => {
                        moved_by_hand = true;
                        let t = ((pp.x - overview.left()) / overview.width()) as f64 * dur;
                        d.view.start = (t - d.view.len / 2.0).max(0.0);
                    }
                    WaveDrag::Select { anchor } => {
                        let t = drag_t.unwrap_or(0.0).clamp(0.0, dur);
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
                } else if on_edge.is_none() {
                    // A click on a selection edge keeps the selection.
                    // While playing, only the ruler moves the playhead.
                    if !playing {
                        d.cursor = x_to_time(&d.view, body, pp.x).clamp(0.0, dur);
                    }
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
    // A time selection or a click outside the spectral tools ends the
    // spectral selection.
    let on_overview = resp
        .interact_pointer_pos()
        .is_some_and(|pp| overview.contains(pp));
    if (resp.drag_started() || resp.clicked()) && !on_overview {
        if let Some(d) = st.doc_mut() {
            d.spec_sel.clear();
        }
    }
    // Right-click on the spectrum: its view menu.
    if let Some(sa) = spec_area {
        spectrum_menu(ui, st, spec_resp.as_ref().unwrap_or(&resp), sa);
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

    let tc = st.time_ctx();
    crate::ruler::draw_ruler(&p, ruler, &v, &th, &d.markers, &tc);

    // Waveform lanes.
    p.rect_filled(wave_area, Rounding::ZERO, c(th.wave_background));
    let step = tc.ruler_step(v.len, wave_area.width());
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
        // dB scale at the right edge, following the amplitude zoom:
        // zoomed in, the quieter marks get room and show.
        let mut last_label = f32::NEG_INFINITY;
        for (db, g) in level_marks(d.amp, lane.height() * 0.46) {
            if g > lane.height() * 0.5 {
                continue;
            }
            for y in [lane.center().y - g, lane.center().y + g] {
                p.line_segment(
                    [pos2(lane.right() - 6.0, y), pos2(lane.right(), y)],
                    Stroke::new(1.0_f32, c(th.border)),
                );
            }
            // Labels above the centre line, at least 11 px apart.
            let y = lane.center().y - g;
            if (y - last_label).abs() >= 11.0 && y > lane.top() + 5.0 {
                p.text(
                    pos2(lane.right() - 8.0, y),
                    Align2::RIGHT_CENTER,
                    if db == 0.0 {
                        "0".to_string()
                    } else {
                        format!("{db:.0}")
                    },
                    FontId::proportional(8.5),
                    ca(th.text_dim, 170),
                );
                last_label = y;
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
        crate::spectral_select::draw(ui, st, sa, &p);
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
    // Start marker, the ruler drag's target, the playhead.
    if let Some(s) = st.start_marker {
        let x = time_to_x(&v, body, s);
        if x >= body.left() && x <= body.right() {
            crate::ruler::start_marker(&p, x, ruler, body.bottom(), &th);
        }
    }
    if let Some(g) = ghost {
        crate::ruler::ghost(&p, time_to_x(&v, body, g), ruler.top(), body.bottom(), &th);
    }
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
                tc.format(t),
                FontId::monospace(10.0),
                ca(th.text, 200),
            );
        }
    }
    if let Some(sa) = spec_area {
        crate::spectral_select::action_bar(ui, st, sa);
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

/// The level scale's marks for amplitude zoom `amp` in a half-lane
/// `half` pixels tall: (dB, pixels from the centre line).
pub fn level_marks(amp: f32, half: f32) -> Vec<(f32, f32)> {
    [0.0f32, -3.0, -6.0, -9.0, -12.0, -18.0, -24.0]
        .iter()
        .map(|&db| {
            (
                db,
                10f32.powf(db / 20.0) * amp.clamp(1.0, crate::zoom::MAX_AMP) * half,
            )
        })
        .collect()
}

/// The frequency under `y` on the spectrum `sa` (the band it is in).
fn freq_at_y(st: &State, sa: Rect, y: f32, nch: usize, ny: f64) -> f64 {
    let nch = nch.clamp(1, 8);
    let lane_h = sa.height() / nch as f32;
    let k = (((y - sa.top()) / lane_h).floor().max(0.0) as usize).min(nch - 1);
    let bottom = sa.top() + lane_h * (k + 1) as f32;
    let frac = ((bottom - y) / lane_h).clamp(0.0, 1.0) as f64;
    st.spec.freq_at(frac, ny)
}

/// Dragging the frequency scale up and down slides through the
/// frequencies at the same zoom (only when zoomed in).
fn freq_scale_drag(ui: &egui::Ui, st: &mut State, sa: Rect) {
    let scale = Rect::from_min_max(pos2(sa.right() - SCALE_W, sa.top()), sa.max);
    let r = ui.interact(scale, ui.id().with("af2-freq-scale"), Sense::drag());
    if !st.spec.zoomed() {
        return;
    }
    if r.hovered() || r.dragged() {
        ui.ctx().set_cursor_icon(if r.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    if r.dragged() {
        let nch = st.doc().map_or(1, |d| d.buf.num_channels()).clamp(1, 8);
        let band = sa.height() / nch as f32;
        let ny = st.doc().map_or(24_000.0, |d| d.buf.rate as f64 / 2.0);
        let dy = r.drag_delta().y;
        if dy != 0.0 {
            st.spec.pan((dy / band.max(1.0)) as f64, ny);
        }
    }
}

/// The mouse wheel over the Waveform editor: the table in
/// [`crate::zoom::wheel_action`] picks what it does.
fn wheel(st: &mut State, a: &Areas, hp: Pos2, w: crate::zoom::WheelInput) {
    use crate::zoom::{Anchor, Region, WheelAction};
    let region = if a.overview.contains(hp) {
        Region::Overview
    } else if a.ruler.contains(hp) {
        Region::WaveRuler
    } else if a
        .spec
        .is_some_and(|sa| sa.contains(hp) && hp.x >= sa.right() - SCALE_W)
    {
        Region::FreqScale
    } else {
        Region::Wave
    };
    let prefs = st.zoom;
    let pos = st.position();
    let lim = st.doc_limits();
    let notches = w.notches();
    let action = crate::zoom::wheel_action(region, w.mods, w.sideways, prefs.wheel_zooms_waveform);
    if action == WheelAction::ZoomFreq {
        let Some(sa) = a.spec else { return };
        let Some(d) = st.doc() else { return };
        let (nch, ny) = (d.buf.num_channels(), d.buf.rate as f64 / 2.0);
        let f = freq_at_y(st, sa, hp.y, nch, ny);
        st.spec.zoom(crate::zoom::freq_wheel_factor(notches), f, ny);
        return;
    }
    let Some(d) = st.doc_mut() else { return };
    let dur = d.duration();
    match action {
        WheelAction::ZoomTime(anchor) => {
            // The time under the pointer: on the overview, its place in
            // the file (held inside the view).
            let pointer_t = if region == Region::Overview {
                let t = ((hp.x - a.overview.left()) / a.overview.width()) as f64 * dur;
                t.clamp(d.view.start, d.view.start + d.view.len)
            } else {
                x_to_time(&d.view, a.body, hp.x.min(a.body.right()))
            };
            let t = match anchor {
                Anchor::Pointer => pointer_t,
                Anchor::Setting => {
                    crate::zoom::anchor_time(prefs.wheel_anchor, pointer_t, &d.view, pos)
                }
                Anchor::Center => d.view.start + d.view.len / 2.0,
            };
            d.view.zoom_at(prefs.wheel_factor(w.points), t, &lim);
        }
        WheelAction::ScrollTime => {
            // A plain wheel with zoom off scrolls with its up and down.
            let px = if w.sideways || w.mods.shift {
                w.scroll[0]
            } else {
                w.scroll[1]
            };
            d.view.pan_px(-px as f64, a.body.width() as f64, &lim);
        }
        WheelAction::ZoomAmp => {
            if a.wave.contains(hp)
                || region == Region::WaveRuler
                || a.spec.is_some_and(|s| s.contains(hp))
            {
                d.amp = crate::zoom::wheel_amp(d.amp, notches);
            }
            return;
        }
        _ => return,
    }
    st.hand_moved_view();
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
        if ui.button("Zoom In (Frequency)").clicked() {
            s.zoom_step(true, ny);
            ui.close_menu();
        }
        if ui.button("Zoom Out (Frequency)").clicked() {
            s.zoom_step(false, ny);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                s.range.is_some(),
                egui::Button::new("Zoom Reset (Frequency)"),
            )
            .clicked()
        {
            s.reset_zoom();
            ui.close_menu();
        }
        ui.separator();
        if ui.radio(!s.notes, "Hertz").clicked() {
            s.notes = false;
            ui.close_menu();
        }
        if ui.radio(s.notes, "Notes").clicked() {
            s.notes = true;
            ui.close_menu();
        }
        ui.separator();
        let full_log = s.log_step == crate::zoom::LOG_STEPS;
        let full_lin = s.log_step == 0;
        if ui
            .add_enabled(!full_log, egui::Button::new("More Logarithmic"))
            .clicked()
        {
            s.more_log();
        }
        if ui
            .add_enabled(!full_lin, egui::Button::new("More Linear"))
            .clicked()
        {
            s.more_linear();
        }
        if ui
            .add_enabled(!full_log, egui::Button::new("Full Logarithmic"))
            .clicked()
        {
            s.log_step = crate::zoom::LOG_STEPS;
            ui.close_menu();
        }
        if ui
            .add_enabled(!full_lin, egui::Button::new("Full Linear"))
            .clicked()
        {
            s.log_step = 0;
            ui.close_menu();
        }
        ui.separator();
        if ui
            .add_enabled(
                s.fft < crate::zoom::FFT_MAX,
                egui::Button::new("Increase Spectral Resolution"),
            )
            .clicked()
        {
            s.finer();
        }
        if ui
            .add_enabled(
                s.fft > crate::zoom::FFT_MIN,
                egui::Button::new("Decrease Spectral Resolution"),
            )
            .clicked()
        {
            s.coarser();
        }
        ui.label(
            egui::RichText::new(format!("Analysis frame: {} samples", s.fft))
                .small()
                .weak(),
        );
        ui.separator();
        crate::spectral_select::colors_menu(ui, st);
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
    // The head on the ruler and the line on the same pixel column.
    let x = crate::ruler::pixel_x(p, x);
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
