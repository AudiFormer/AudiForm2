// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.
// The graph editor's design and maths follow the Parametric EQ graph its
// author (AudiFormer) designed for Audiform; rewritten here in Rust/egui.

//! The Parametric EQ's window: its response curve over the sound itself.
//!
//! Each band is a node to drag, left and right for frequency, up and down
//! for gain (the high-pass and low-pass move sideways only). A band's
//! width (Q) shows as two handles either side of its node: drag one, turn
//! the mouse wheel over the node, or hold Alt and drag the node up and
//! down. Double-click a node to switch its band on or off. Behind the
//! curve, the spectrum of what is playing (while stopped, of the audio at
//! the playhead). The slider on the left is the output gain. Below the
//! graph, a column per band: frequency, gain (or slope for the pass
//! filters), width as Q or in octaves, and the band's switch. Range shows
//! 30 dB or 96 dB on the graph. Every drag is one undoable step.

use std::cell::RefCell;

use af2_fx::effects::parametric_eq::{self as eq, Kind, BANDS, SLOPES, SLOPE_ORDER};
use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Pos2, Rect, RichText, Sense, Shape, Stroke};

use crate::rack::{chain_of, FILE};
use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};

/// The graph's frequency span, evenly in pitch.
pub const LOW: f64 = 20.0;
pub const HIGH: f64 = 20_000.0;

/// Whether a slot holds the Parametric EQ (its window is this editor).
pub fn is_eq(effect: &str) -> bool {
    effect == eq::ID
}

/// The window's default size.
pub const DEFAULT_SIZE: [f32; 2] = [780.0, 520.0];

// ------------------------------------------------------------- the maths

/// 0 to 1 across the graph.
pub fn x_of(freq: f64) -> f64 {
    (freq.clamp(LOW, HIGH) / LOW).ln() / (HIGH / LOW).ln()
}

pub fn freq_at(x: f64) -> f64 {
    LOW * (HIGH / LOW).powf(x.clamp(0.0, 1.0))
}

/// A peaking band's width in octaves for a Q.
pub fn octaves_of(q: f64) -> f64 {
    (2.0 / std::f64::consts::LN_2) * (1.0 / (2.0 * q.max(1e-3))).asinh()
}

/// The Q for a width in octaves.
pub fn q_of(octaves: f64) -> f64 {
    let r = 2f64.powf(octaves.max(0.01));
    r.sqrt() / (r - 1.0)
}

/// Where a band's width handles sit: half its width either side.
pub fn width_edges(freq: f64, q: f64) -> [f64; 2] {
    let half = 2f64.powf(octaves_of(q) / 2.0);
    [freq / half, freq * half]
}

/// The Q a width handle dragged to `at` Hz gives a band centred on `freq`.
pub fn q_from_edge(freq: f64, at: f64) -> f64 {
    q_of(2.0 * (at.max(1.0) / freq.max(1.0)).log2().abs())
}

/// "1.2 kHz" or "80 Hz".
pub fn format_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        let s = if hz >= 10_000.0 {
            format!("{k:.1}")
        } else {
            format!("{k:.2}")
        };
        let s = s.trim_end_matches('0').trim_end_matches('.');
        format!("{s} kHz")
    } else {
        format!("{} Hz", hz.round())
    }
}

/// "1.2k", "1200", "1.2 kHz" or "80 Hz" back to hertz.
pub fn parse_hz(text: &str) -> Option<f64> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.trim_end_matches("hz").trim();
    let (num, mul) = match t.strip_suffix('k') {
        Some(n) => (n.trim(), 1000.0),
        None => (t, 1.0),
    };
    num.parse::<f64>().ok().map(|v| v * mul)
}

/// A line on the frequency ruler.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub f: f64,
    /// Its label ("Hz" at the left end, "1k" and so on), or None when it
    /// is a line only.
    pub label: Option<String>,
    /// 100 Hz, 1 kHz and 10 kHz: drawn stronger.
    pub major: bool,
}

/// The ruler for a graph `width` points wide: every 1 to 9 step of each
/// decade, labelled where there is room (the round ones always).
pub fn freq_ticks(width: f64) -> Vec<Tick> {
    let mut out = Vec::new();
    let mut last_x = f64::NEG_INFINITY;
    for decade in [10.0, 100.0, 1000.0, 10_000.0] {
        for k in 1..=9 {
            let f = decade * k as f64;
            if !(LOW..=HIGH).contains(&f) {
                continue;
            }
            let x = x_of(f) * width;
            let major = k == 1 && decade >= 100.0;
            let ends = f == LOW || f == HIGH;
            let text = if f == LOW {
                "Hz".to_string()
            } else if f >= 1000.0 {
                format!("{}k", f / 1000.0)
            } else {
                format!("{f}")
            };
            let room = if major || ends {
                0.0
            } else {
                20.0 + text.len() as f64 * 2.0
            };
            let fits = x <= width - 14.0 + if f == HIGH { 14.0 } else { 0.0 };
            let label = (x - last_x >= room && fits).then_some(text);
            if label.is_some() {
                last_x = x;
            }
            out.push(Tick { f, label, major });
        }
    }
    out
}

// ------------------------------------------------------------ the window

/// How the editor shows things, shared by every EQ window.
#[derive(Debug, Clone, Copy, Default)]
struct View {
    /// 96 dB (±48) instead of 30 dB (±15).
    wide: bool,
    /// Width in octaves instead of Q.
    octaves: bool,
}

/// What a drag on the graph holds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    Node(usize),
    /// A width handle of a band, with the band's frequency.
    Width(usize, f64),
}

thread_local! {
    /// The spectrum reader and the levels shown (falling back slowly).
    static SPECTRUM: RefCell<(crate::visualizer::Spectrum, Vec<f32>, Vec<f32>)> =
        RefCell::new((Default::default(), vec![-120.0; POINTS], vec![-120.0; POINTS]));
}

/// Spectrum bands drawn behind the curve.
const POINTS: usize = 200;
/// The spectrum's range, top and bottom (dBFS).
const S_TOP: f32 = 0.0;
const S_BOTTOM: f32 = -96.0;

/// The settings being edited: the slot's values, by key.
struct Values {
    info: af2_fx::EffectInfo,
    vals: Vec<f32>,
    changed: bool,
    released: bool,
    label: String,
}

impl Values {
    fn index(&self, key: &str) -> Option<usize> {
        self.info.params.iter().position(|p| p.key == key)
    }

    fn get(&self, key: &str) -> f64 {
        self.index(key).map_or(0.0, |i| self.vals[i] as f64)
    }

    fn on(&self, key: &str) -> bool {
        self.get(key) >= 0.5
    }

    fn range(&self, key: &str) -> (f64, f64) {
        self.info.param(key).map_or((0.0, 1.0), |p| (p.min, p.max))
    }

    /// Sets a value (kept in range); `label` names the undo step.
    fn set(&mut self, key: &str, v: f64, label: &str) {
        let Some(i) = self.index(key) else {
            return;
        };
        let v = self.info.params[i].clamp(v) as f32;
        if self.vals[i] != v {
            self.vals[i] = v;
            self.changed = true;
            if self.label.is_empty() {
                self.label = label.to_string();
            }
        }
    }

    fn params(&self) -> af2_fx::Params {
        let pairs: Vec<(String, f64)> = self
            .info
            .params
            .iter()
            .zip(&self.vals)
            .map(|(p, v)| (p.key.to_string(), *v as f64))
            .collect();
        af2_fx::Params::from_pairs(&self.info, &pairs)
    }
}

fn band_color(th: &Theme, b: usize) -> Color32 {
    if th.track_colors.is_empty() {
        c(th.accent)
    } else {
        c(th.track_colors[b % th.track_colors.len()])
    }
}

/// The Parametric EQ editor for slot `i` of rack `target`.
pub fn show(ui: &mut egui::Ui, st: &mut State, target: u16, i: usize, th: &Theme) {
    let Some(slot) = chain_of(st, target).and_then(|c| c.get(i)).cloned() else {
        return;
    };
    let Some(info) = af2_fx::info(eq::ID) else {
        return;
    };
    let mut vals = slot.values.clone();
    for p in info.params.iter().skip(vals.len()) {
        vals.push(p.default as f32);
    }
    let mut v = Values {
        info,
        vals,
        changed: false,
        released: false,
        label: String::new(),
    };
    let rate = if target == FILE {
        st.doc().map_or(48_000, |d| d.buf.rate)
    } else {
        st.session.rate
    };
    let view_id = egui::Id::new("eq-editor-view");
    let mut view: View = ui.data(|d| d.get_temp(view_id)).unwrap_or_default();

    // Graph above, the band table and options below.
    let table_h = 5.0 * 26.0 + 34.0;
    let graph_h = (ui.available_height() - table_h).clamp(170.0, 900.0);
    ui.horizontal(|ui| {
        master(ui, &mut v, graph_h, th);
        let w = ui.available_width().max(320.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, graph_h), Sense::click_and_drag());
        graph(ui, st, &mut v, rect, &resp, rate, view, (target, i), th);
    });
    ui.add_space(6.0);
    table(ui, &mut v, view, (target, i), th);
    ui.add_space(2.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Constant:").small().color(c(th.text_dim)));
        ui.radio_value(&mut view.octaves, false, "Q");
        ui.radio_value(&mut view.octaves, true, "Width");
        ui.separator();
        ui.label(RichText::new("Range:").small().color(c(th.text_dim)));
        ui.radio_value(&mut view.wide, false, "30 dB");
        ui.radio_value(&mut view.wide, true, "96 dB");
    });
    ui.data_mut(|d| d.insert_temp(view_id, view));

    // Into the slot and the running mix, one undo step per gesture.
    if v.changed {
        // For undo; taken only on a change, as a session can carry
        // megabytes of slot data.
        let before = st.session.clone();
        if let Some(s) = chain_of(st, target).and_then(|c| c.get_mut(i)) {
            s.values = v.vals.clone();
        }
        if target == FILE {
            // The Waveform editor plays through the file's rack.
            st.live_file_slot(i);
            st.gesture = ui.input(|i| i.pointer.any_down()) || wheel_pending(ui);
        } else {
            st.live_slot(target, i);
            let down = ui.input(|i| i.pointer.any_down()) || wheel_pending(ui);
            if !st.gesture {
                let label = if v.label.is_empty() {
                    "Parametric EQ".to_string()
                } else {
                    format!("EQ {}", v.label)
                };
                st.commit_live(&label, before);
                st.gesture = down;
            }
        }
    }
    if v.released {
        st.gesture = false;
        // The stopped spectrum is of the mix: read it again.
        if !st.is_playing() {
            st.scope_cache = None;
        }
    }
}

/// The output gain, a vertical slider beside the graph.
fn master(ui: &mut egui::Ui, v: &mut Values, h: f32, th: &Theme) {
    ui.vertical(|ui| {
        ui.set_width(58.0);
        ui.label(RichText::new("Output").small().color(c(th.text_dim)));
        let (lo, hi) = v.range(eq::GAIN);
        let g = v.get(eq::GAIN);
        // A fader: the rail, lit from 0 dB to the value, and its cap.
        let size = vec2(58.0, (h - 46.0).max(60.0));
        let (rect, r) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let rail = Rect::from_center_size(rect.center(), vec2(6.0, rect.height() - 12.0));
        let y_of = |db: f64| rail.bottom() - ((db - lo) / (hi - lo)) as f32 * rail.height();
        let p = ui.painter_at(rect);
        p.rect_filled(rail, 3.0, c(th.panel_raised));
        p.rect_stroke(rail, 3.0, Stroke::new(1.0_f32, c(th.border)));
        let (y0, y1) = (y_of(0.0), y_of(g));
        let lit = Rect::from_x_y_ranges(rail.x_range(), y0.min(y1)..=y0.max(y1));
        p.rect_filled(lit, 2.0, c(th.accent));
        for d in [20.0, 10.0, 0.0, -10.0, -20.0, -30.0, -40.0] {
            let y = y_of(d);
            let w = if d == 0.0 { 9.0 } else { 5.0 };
            p.line_segment(
                [pos2(rail.left() - 3.0 - w, y), pos2(rail.left() - 3.0, y)],
                Stroke::new(1.0_f32, c(th.text_dim)),
            );
        }
        let cap = Rect::from_center_size(pos2(rail.center().x, y1), vec2(24.0, 10.0));
        let hot = r.hovered() || r.dragged();
        p.rect_filled(cap, 2.0, if hot { c(th.accent) } else { c(th.text) });
        p.line_segment(
            [pos2(cap.left() + 3.0, y1), pos2(cap.right() - 3.0, y1)],
            Stroke::new(1.0_f32, c(th.wave_background)),
        );
        let r = r.on_hover_text("Output gain (double-click: 0 dB)");
        if r.double_clicked() {
            v.set(eq::GAIN, 0.0, "Output");
            v.released = true;
        } else if r.dragged() {
            if let Some(at) = r.interact_pointer_pos() {
                let db = lo + ((rail.bottom() - at.y) / rail.height()) as f64 * (hi - lo);
                v.set(eq::GAIN, (db * 10.0).round() / 10.0, "Output");
            }
        }
        v.released |= r.drag_stopped();
        let mut typed = g;
        let f = ui.add_sized(
            vec2(56.0, 20.0),
            egui::DragValue::new(&mut typed)
                .range(lo..=hi)
                .speed(0.1)
                .fixed_decimals(1)
                .suffix(" dB"),
        );
        if f.changed() {
            v.set(eq::GAIN, typed, "Output");
        }
        done(v, &f);
    });
}

/// Wheel turns on a node finish as one step once they stop.
const WHEEL_DONE: f64 = 0.35;

fn wheel_pending(ui: &egui::Ui) -> bool {
    ui.data(|d| d.get_temp::<f64>(egui::Id::new("eq-editor-wheel")))
        .is_some()
}

#[allow(clippy::too_many_arguments)]
fn graph(
    ui: &mut egui::Ui,
    st: &mut State,
    v: &mut Values,
    rect: Rect,
    resp: &egui::Response,
    rate: u32,
    view: View,
    key: (u16, usize),
    th: &Theme,
) {
    let painter = ui.painter_at(rect);
    // Room for the ruler below and the dB labels on the right.
    let plot = Rect::from_min_max(rect.min + vec2(4.0, 8.0), rect.max - vec2(34.0, 18.0));
    let (top, bottom) = if view.wide {
        (48.0, -48.0)
    } else {
        (15.0, -15.0)
    };
    let px = |f: f64| plot.left() + x_of(f) as f32 * plot.width();
    let py = |db: f64| {
        let d = db.clamp(bottom - 6.0, top + 6.0);
        plot.top() + ((top - d) / (top - bottom)) as f32 * plot.height()
    };
    let hz_at = |x: f32| freq_at(((x - plot.left()) / plot.width()) as f64);
    let db_at = |y: f32| top - ((y - plot.top()) / plot.height()) as f64 * (top - bottom);

    painter.rect_filled(plot, th.rounding.min(4.0), c(th.wave_background));

    // The sound behind it.
    let mut levels = [0f32; POINTS];
    SPECTRUM.with(|s| {
        let (sp, fresh, shown) = &mut *s.borrow_mut();
        st.scope_spectrum(sp, fresh);
        for ((o, n), l) in shown.iter_mut().zip(fresh.iter()).zip(levels.iter_mut()) {
            *o = n.max(*o - 1.5);
            *l = *o;
        }
    });
    let sy = |db: f32| {
        plot.top() + ((S_TOP - db.clamp(S_BOTTOM, S_TOP)) / (S_TOP - S_BOTTOM)) * plot.height()
    };
    let mut mesh = Mesh::default();
    let fill = ca(th.accent, 46);
    for (k, l) in levels.iter().enumerate() {
        let f = LOW * 1000f64.powf(k as f64 / (POINTS - 1) as f64);
        let x = px(f);
        mesh.colored_vertex(pos2(x, sy(*l)), fill);
        mesh.colored_vertex(pos2(x, plot.bottom()), ca(th.accent, 8));
        if k > 0 {
            let b = (2 * k) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    painter.add(Shape::mesh(mesh));
    let spectrum_line: Vec<Pos2> = levels
        .iter()
        .enumerate()
        .map(|(k, l)| {
            pos2(
                px(LOW * 1000f64.powf(k as f64 / (POINTS - 1) as f64)),
                sy(*l),
            )
        })
        .collect();
    painter.add(Shape::line(
        spectrum_line,
        Stroke::new(0.8_f32, ca(th.accent, 110)),
    ));

    // The ruler and the level grid.
    let grid = c(th.grid);
    let major = lerp(th.grid, th.text, 0.18);
    let zero = lerp(th.grid, th.text, 0.4);
    let font = FontId::proportional(9.5);
    let label_y = rect.bottom() - 3.0;
    for t in freq_ticks(plot.width() as f64) {
        let x = px(t.f);
        painter.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(1.0_f32, if t.major { major } else { grid }),
        );
        if let Some(text) = t.label {
            let align = if t.f == LOW {
                Align2::LEFT_BOTTOM
            } else if t.f == HIGH {
                Align2::RIGHT_BOTTOM
            } else {
                Align2::CENTER_BOTTOM
            };
            let col = if t.major { c(th.text) } else { c(th.text_dim) };
            painter.text(pos2(x, label_y), align, text, font.clone(), col);
        }
    }
    let steps: &[f64] = if view.wide {
        &[48.0, 36.0, 24.0, 12.0, 0.0, -12.0, -24.0, -36.0, -48.0]
    } else {
        &[15.0, 10.0, 5.0, 0.0, -5.0, -10.0, -15.0]
    };
    for &d in steps {
        let y = py(d);
        painter.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0_f32, if d == 0.0 { zero } else { grid }),
        );
        let text = if d > 0.0 {
            format!("+{d}")
        } else {
            format!("{d}")
        };
        painter.text(
            pos2(plot.right() + 4.0, y),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            c(th.text_dim),
        );
    }
    painter.text(
        pos2(plot.right() + 4.0, label_y),
        Align2::LEFT_BOTTOM,
        "dB",
        font.clone(),
        c(th.text_dim),
    );

    // Where each band's node sits.
    struct Placed {
        f: f64,
        g: f64,
        q: Option<f64>,
        on: bool,
        pos: Pos2,
        edges: Option<[f64; 2]>,
    }
    let placed: Vec<Placed> = BANDS
        .iter()
        .map(|b| {
            let f = v.get(b.freq);
            let g = b.gain.map_or(0.0, |k| v.get(k));
            let q = b.q.map(|k| v.get(k));
            let y = py(g).clamp(plot.top(), plot.bottom());
            Placed {
                f,
                g,
                q,
                on: v.on(b.on),
                pos: pos2(px(f), y),
                edges: q.map(|q| width_edges(f, q)),
            }
        })
        .collect();

    // What the pointer is on: a width handle or a node, nearest first.
    let hit = |at: Pos2| -> Option<Grab> {
        let mut best: Option<(f32, Grab)> = None;
        for (b, p) in placed.iter().enumerate() {
            let d = p.pos.distance(at);
            if d <= 11.0 && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, Grab::Node(b)));
            }
            if let (Some(e), true) = (p.edges, p.on) {
                for f in e {
                    let d = pos2(px(f), p.pos.y).distance(at) + 2.0;
                    if d <= 8.0 && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, Grab::Width(b, p.f)));
                    }
                }
            }
        }
        best.map(|(_, g)| g)
    };
    let grab_id = egui::Id::new(("eq-editor-grab", key));
    let mut grab: Option<Grab> = ui.data(|d| d.get_temp(grab_id)).flatten();
    if resp.drag_started() {
        grab = resp.interact_pointer_pos().and_then(hit);
    }
    let alt = ui.input(|i| i.modifiers.alt);
    if resp.dragged() {
        if let (Some(g), Some(at)) = (grab, resp.interact_pointer_pos()) {
            match g {
                Grab::Node(b) => {
                    let band = &BANDS[b];
                    if alt {
                        // Alt: up narrower, down wider.
                        if let Some(qk) = band.q {
                            let dy = resp.drag_delta().y as f64;
                            let q = v.get(qk) * 2f64.powf(-dy / 60.0);
                            v.set(
                                qk,
                                (q * 100.0).round() / 100.0,
                                &format!("{} Width", band.name),
                            );
                        }
                    } else {
                        let name = format!("{} Point", band.name);
                        v.set(band.freq, hz_at(at.x).round(), &name);
                        if let Some(gk) = band.gain {
                            v.set(gk, (db_at(at.y) * 10.0).round() / 10.0, &name);
                        }
                    }
                }
                Grab::Width(b, f) => {
                    let band = &BANDS[b];
                    if let Some(qk) = band.q {
                        let q = q_from_edge(f, hz_at(at.x));
                        v.set(
                            qk,
                            (q * 100.0).round() / 100.0,
                            &format!("{} Width", band.name),
                        );
                    }
                }
            }
        }
    }
    if resp.drag_stopped() {
        grab = None;
        v.released = true;
    }
    if resp.double_clicked() {
        if let Some(Grab::Node(b)) = resp.interact_pointer_pos().and_then(hit) {
            let band = &BANDS[b];
            let on = v.on(band.on);
            v.set(
                band.on,
                if on { 0.0 } else { 1.0 },
                &format!("{} {}", band.name, if on { "Off" } else { "On" }),
            );
            v.released = true;
        }
    }
    // The wheel over a node: narrower (up) or wider (down).
    let hover = resp.hover_pos().and_then(hit);
    let wheel_id = egui::Id::new("eq-editor-wheel");
    let now = ui.input(|i| i.time);
    if let Some(Grab::Node(b)) = hover {
        let dy = ui.input(|i| i.raw_scroll_delta.y);
        if let (Some(qk), true) = (BANDS[b].q, dy != 0.0) {
            let q = v.get(qk) * if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
            v.set(
                qk,
                (q * 100.0).round() / 100.0,
                &format!("{} Width", BANDS[b].name),
            );
            ui.data_mut(|d| d.insert_temp(wheel_id, now));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(WHEEL_DONE + 0.05));
        }
    }
    if let Some(t) = ui.data(|d| d.get_temp::<f64>(wheel_id)) {
        if now - t > WHEEL_DONE {
            ui.data_mut(|d| d.remove::<f64>(wheel_id));
            v.released = true;
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(WHEEL_DONE));
        }
    }
    ui.data_mut(|d| d.insert_temp(grab_id, grab));
    let focus = grab.or(hover).map(|g| match g {
        Grab::Node(b) | Grab::Width(b, _) => b,
    });
    if focus.is_some() {
        ui.ctx().set_cursor_icon(match grab.or(hover) {
            Some(Grab::Width(..)) => egui::CursorIcon::ResizeHorizontal,
            _ => egui::CursorIcon::Move,
        });
    }

    // The curve (every band on, plus the output gain), after this frame's edits.
    let params = v.params();
    let n = (plot.width() as usize / 2).clamp(120, 480);
    let freqs: Vec<f64> = (0..=n).map(|k| freq_at(k as f64 / n as f64)).collect();
    let clip = painter.with_clip_rect(plot);
    if let Some(b) = focus {
        if v.on(BANDS[b].on) {
            let col = band_color(th, b);
            let pts: Vec<Pos2> = freqs
                .iter()
                .map(|&f| pos2(px(f), py(eq::band_response_db(&params, rate, b, f))))
                .collect();
            clip.add(Shape::line(
                pts,
                Stroke::new(1.2_f32, col.gamma_multiply(0.8)),
            ));
        }
    }
    let curve: Vec<Pos2> = freqs
        .iter()
        .map(|&f| pos2(px(f), py(eq::response_db(&params, rate, f))))
        .collect();
    let mut mesh = Mesh::default();
    let y0 = py(0.0);
    let tint = ca(th.accent_alt, 34);
    for (k, p) in curve.iter().enumerate() {
        mesh.colored_vertex(*p, tint);
        mesh.colored_vertex(pos2(p.x, y0), tint);
        if k > 0 {
            let b = (2 * k) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    clip.add(Shape::mesh(mesh));
    if th.glow > 0.0 {
        clip.add(Shape::line(
            curve.clone(),
            Stroke::new(5.0_f32, ca(th.accent, (50.0 * th.glow.min(1.0)) as u8)),
        ));
    }
    clip.add(Shape::line(curve, Stroke::new(2.0_f32, c(th.accent))));

    // Nodes and width handles, over the curve.
    for (b, p) in placed.iter().enumerate() {
        let band = &BANDS[b];
        let col = band_color(th, b);
        let lit = focus == Some(b);
        let y = if band.gain.is_some() {
            p.pos.y
        } else {
            py(0.0)
        };
        if let (Some(e), true) = (p.edges, p.on) {
            let (x0, x1) = (px(e[0]), px(e[1]));
            clip.extend(Shape::dashed_line(
                &[pos2(x0, y), pos2(x1, y)],
                Stroke::new(1.0_f32, col.gamma_multiply(0.6)),
                2.0,
                2.0,
            ));
            for x in [x0, x1] {
                let r = Rect::from_center_size(pos2(x, y), vec2(5.0, 12.0));
                clip.rect_filled(r, 1.0, if lit { col } else { col.gamma_multiply(0.55) });
            }
        }
        let r = Rect::from_center_size(pos2(p.pos.x, y), vec2(10.0, 10.0));
        if p.on {
            clip.rect_filled(r, 2.0, col);
            clip.rect_stroke(r, 2.0, Stroke::new(1.0_f32, c(th.wave_background)));
        } else {
            clip.rect_stroke(r, 2.0, Stroke::new(1.0_f32, col.gamma_multiply(0.6)));
        }
        if lit {
            clip.rect_stroke(r.expand(3.0), 3.0, Stroke::new(1.0_f32, col));
        }
        clip.text(
            pos2(p.pos.x, y - 8.0),
            Align2::CENTER_BOTTOM,
            band.label,
            FontId::proportional(10.0),
            if p.on { c(th.text) } else { c(th.text_dim) },
        );
    }

    // What the pointer is on, in words.
    if let Some(b) = focus {
        let p = &placed[b];
        let band = &BANDS[b];
        let mut parts = vec![band.name.to_string(), format_hz(p.f)];
        if band.gain.is_some() {
            parts.push(format!("{:+.1} dB", p.g));
        }
        if let Some(q) = p.q {
            if view.octaves {
                parts.push(format!("{:.2} oct", octaves_of(q)));
            } else {
                parts.push(format!("Q {q:.2}"));
            }
        }
        if let Some(sk) = band.slope {
            parts.push(SLOPES[(v.get(sk).round() as usize).min(SLOPES.len() - 1)].to_string());
        }
        if !p.on {
            parts.push("off".into());
        }
        let text = parts.join(" \u{b7} ");
        let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), c(th.text));
        let size = galley.size() + vec2(10.0, 4.0);
        let x = (p.pos.x - size.x / 2.0).clamp(plot.left() + 2.0, plot.right() - size.x - 2.0);
        let mut y = p.pos.y - 34.0;
        if y < plot.top() + 2.0 {
            y = p.pos.y + 14.0;
        }
        let r = Rect::from_min_size(pos2(x, y), size);
        painter.rect_filled(r, 3.0, c(th.panel_raised));
        painter.rect_stroke(r, 3.0, Stroke::new(1.0_f32, c(th.border)));
        painter.galley(r.min + vec2(5.0, 2.0), galley, c(th.text));
    }
    painter.rect_stroke(
        plot,
        th.rounding.min(4.0),
        Stroke::new(1.0_f32, c(th.border)),
    );
}

/// A column per band: frequency, gain or slope, width, and its switch.
fn table(ui: &mut egui::Ui, v: &mut Values, view: View, key: (u16, usize), th: &Theme) {
    let head_w = 70.0;
    let n = BANDS.len() as f32;
    let col_w = ((ui.available_width() - head_w - 2.0 * n) / n).max(52.0);
    let cell = vec2(col_w, 20.0);
    let head = |ui: &mut egui::Ui, text: &str| {
        ui.add_sized(
            vec2(head_w, 20.0),
            egui::Label::new(RichText::new(text).small().color(c(th.text_dim))),
        );
    };
    egui::Grid::new(("eq-table", key))
        .num_columns(BANDS.len() + 1)
        .spacing(vec2(2.0, 5.0))
        .min_col_width(0.0)
        .show(ui, |ui| {
            head(ui, "Frequency");
            for b in BANDS {
                let on = v.on(b.on);
                ui.scope(|ui| {
                    if !on {
                        ui.multiply_opacity(0.45);
                    }
                    let (lo, hi) = v.range(b.freq);
                    let mut f = v.get(b.freq);
                    let speed = f * 0.004;
                    let r = ui.add_sized(
                        cell,
                        egui::DragValue::new(&mut f)
                            .range(lo..=hi)
                            .speed(speed)
                            .custom_formatter(|x, _| format_hz(x))
                            .custom_parser(parse_hz),
                    );
                    if r.changed() {
                        v.set(b.freq, f.round(), &format!("{} Frequency", b.name));
                    }
                    done(v, &r);
                });
            }
            ui.end_row();

            head(ui, "Gain");
            for b in BANDS {
                let on = v.on(b.on);
                ui.scope(|ui| {
                    if !on {
                        ui.multiply_opacity(0.45);
                    }
                    if let Some(gk) = b.gain {
                        let (lo, hi) = v.range(gk);
                        let mut g = v.get(gk);
                        let r = ui.add_sized(
                            cell,
                            egui::DragValue::new(&mut g)
                                .range(lo..=hi)
                                .speed(0.1)
                                .fixed_decimals(1)
                                .suffix(" dB"),
                        );
                        if r.changed() {
                            v.set(gk, g, &format!("{} Gain", b.name));
                        }
                        done(v, &r);
                    } else if let Some(sk) = b.slope {
                        let cur = (v.get(sk).round() as usize).min(SLOPES.len() - 1);
                        let mut pick = cur;
                        let short = |k: usize| SLOPES[k].replace(" dB/oct", " dB");
                        egui::ComboBox::from_id_salt(("eq-slope", key, b.label))
                            .selected_text(short(cur))
                            .width(col_w - 8.0)
                            .show_ui(ui, |ui| {
                                for k in SLOPE_ORDER {
                                    ui.selectable_value(&mut pick, k, SLOPES[k]);
                                }
                            })
                            .response
                            .on_hover_text("Slope, per octave");
                        if pick != cur {
                            v.set(sk, pick as f64, &format!("{} Slope", b.name));
                            v.released = true;
                        }
                    }
                });
            }
            ui.end_row();

            head(ui, if view.octaves { "Width" } else { "Q" });
            for b in BANDS {
                let on = v.on(b.on);
                ui.scope(|ui| {
                    if !on {
                        ui.multiply_opacity(0.45);
                    }
                    if let Some(qk) = b.q {
                        let (lo, hi) = v.range(qk);
                        let q = v.get(qk);
                        let label = format!("{} Width", b.name);
                        let r = if view.octaves {
                            let mut o = octaves_of(q);
                            let r = ui.add_sized(
                                cell,
                                egui::DragValue::new(&mut o)
                                    .range(octaves_of(hi)..=octaves_of(lo).min(8.0))
                                    .speed(0.01)
                                    .fixed_decimals(2)
                                    .suffix(" oct"),
                            );
                            if r.changed() {
                                v.set(qk, q_of(o), &label);
                            }
                            r
                        } else {
                            let mut q2 = q;
                            let r = ui.add_sized(
                                cell,
                                egui::DragValue::new(&mut q2)
                                    .range(lo..=hi)
                                    .speed(q * 0.01)
                                    .fixed_decimals(2),
                            );
                            if r.changed() {
                                v.set(qk, q2, &label);
                            }
                            r
                        };
                        done(v, &r);
                    } else {
                        ui.allocate_exact_size(cell, Sense::hover());
                    }
                });
            }
            ui.end_row();

            head(ui, "Band");
            for (k, b) in BANDS.iter().enumerate() {
                let on = v.on(b.on);
                let col = band_color(th, k);
                let text =
                    RichText::new(b.label)
                        .strong()
                        .color(if on { col } else { c(th.text_dim) });
                let r = ui
                    .add_sized(
                        vec2(col_w.min(48.0), 22.0),
                        egui::Button::new(text)
                            .stroke(Stroke::new(1.0_f32, if on { col } else { c(th.border) }))
                            .fill(c(th.panel)),
                    )
                    .on_hover_text(format!(
                        "{}: {} ({})",
                        b.name,
                        if on { "on" } else { "off" },
                        match b.kind {
                            Kind::HighPass | Kind::LowPass => "filter",
                            Kind::Bell => "bell",
                            _ => "shelf",
                        }
                    ));
                if r.clicked() {
                    v.set(
                        b.on,
                        if on { 0.0 } else { 1.0 },
                        &format!("{} {}", b.name, if on { "Off" } else { "On" }),
                    );
                    v.released = true;
                }
            }
            ui.end_row();
        });
}

/// A field's change is finished: let go, typed, or stepped.
fn done(v: &mut Values, r: &egui::Response) {
    v.released |= r.drag_stopped() || (r.changed() && !r.dragged()) || r.lost_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_runs_evenly_in_pitch() {
        assert_eq!(x_of(LOW), 0.0);
        assert!((x_of(HIGH) - 1.0).abs() < 1e-12);
        assert!((x_of(632.455_532) - 0.5).abs() < 1e-6);
        for f in [20.0, 100.0, 1234.0, 19_999.0] {
            assert!((freq_at(x_of(f)) - f).abs() < 1e-6 * f);
        }
    }

    #[test]
    fn q_and_octaves_turn_into_each_other() {
        // One octave is Q 1.414; Q 1 is 1.39 octaves.
        assert!((q_of(1.0) - std::f64::consts::SQRT_2).abs() < 1e-9);
        assert!((octaves_of(1.0) - 1.3884).abs() < 1e-3);
        for q in [0.3, 1.0, 4.0, 30.0] {
            assert!((q_of(octaves_of(q)) - q).abs() < 1e-6 * q);
            let [lo, hi] = width_edges(1000.0, q);
            assert!((q_from_edge(1000.0, hi) - q).abs() < 1e-6 * q);
            assert!((q_from_edge(1000.0, lo) - q).abs() < 1e-6 * q);
        }
    }

    #[test]
    fn frequencies_read_and_write() {
        assert_eq!(format_hz(80.4), "80 Hz");
        assert_eq!(format_hz(1200.0), "1.2 kHz");
        assert_eq!(format_hz(12_500.0), "12.5 kHz");
        assert_eq!(format_hz(1000.0), "1 kHz");
        assert_eq!(parse_hz("1.2k"), Some(1200.0));
        assert_eq!(parse_hz("1.2 kHz"), Some(1200.0));
        assert_eq!(parse_hz("80 Hz"), Some(80.0));
        assert_eq!(parse_hz("x"), None);
    }

    #[test]
    fn the_ruler_has_every_line_and_the_round_labels() {
        let t = freq_ticks(600.0);
        // 20..90, 100..900, 1k..9k, 10k, 20k.
        assert_eq!(t.len(), 8 + 9 + 9 + 2);
        for f in [100.0, 1000.0, 10_000.0] {
            let k = t.iter().find(|x| x.f == f).unwrap();
            assert!(k.major && k.label.is_some(), "{f}");
        }
        assert_eq!(t[0].label.as_deref(), Some("Hz"));
        assert_eq!(t.last().unwrap().label.as_deref(), Some("20k"));
        // Narrow: fewer labels, the same lines.
        let n = freq_ticks(200.0);
        assert_eq!(n.len(), t.len());
        assert!(
            n.iter().filter(|x| x.label.is_some()).count()
                < t.iter().filter(|x| x.label.is_some()).count()
        );
    }
}
