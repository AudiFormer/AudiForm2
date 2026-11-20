// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Phase Curve's graph editor, after the lead's Audiform window:
//! frequency across (logarithmic or linear, 20 Hz to 20 kHz), the phase
//! turn up and down (±180° or ±360°). Drag a point to move it; click the
//! curve or empty space to add one (up to sixteen); right-click or
//! double-click a point to remove it (two always stay). Below: Spline,
//! Reset, the scale, range, channel and FFT size. Each change is one
//! undo step.
//!
//! The curve is worked out with the effect's own `curve_at`, so what is
//! drawn is what is heard, and only again when a value changes.

use std::sync::Arc;

use egui::{pos2, vec2, Pos2, RichText, Sense};

use af27_fx::effects::phase_curve::{curve_at, FREQ_KEYS, MAX_POINTS, PHASE_KEYS};

use super::{Env, Fx};
use crate::graph::{self, Plot};
use crate::theme::c;

/// Where unused points start (as the effect's own defaults).
const SPREAD: [f64; MAX_POINTS] = [
    20.0, 20_000.0, 100.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 50.0, 200.0, 300.0, 700.0,
    1_500.0, 3_000.0, 7_000.0, 15_000.0,
];

/// The points in use as (index, Hz, degrees).
fn points(fx: &Fx, limit: f64) -> Vec<(usize, f64, f64)> {
    let n = (fx.get("points").round() as usize).clamp(2, MAX_POINTS);
    (0..n)
        .map(|i| {
            (
                i,
                fx.get(FREQ_KEYS[i]).max(1.0),
                fx.get(PHASE_KEYS[i]).clamp(-limit, limit),
            )
        })
        .collect()
}

/// The curve sampled across the plot, kept until a value changes.
fn sampled(ui: &egui::Ui, fx: &Fx, plot: &Plot, spline: bool, limit: f64) -> Arc<Vec<(f64, f64)>> {
    let mut h: u64 = if plot.linear { 1 } else { 2 };
    for v in &fx.vals {
        h = h
            .wrapping_mul(0x100_0000_01b3)
            .wrapping_add(u64::from(v.to_bits()));
    }
    let id = fx.uid.with("phase-curve");
    if let Some((k, v)) = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Arc<Vec<(f64, f64)>>)>(id))
    {
        if k == h {
            return v;
        }
    }
    let mut pts: Vec<(f64, f64)> = points(fx, limit).iter().map(|p| (p.1, p.2)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let v: Vec<(f64, f64)> = (0..=200)
        .map(|i| {
            let t = i as f64 / 200.0;
            let hz = if plot.linear {
                (t * plot.f_hi).max(plot.f_lo)
            } else {
                plot.f_lo * (plot.f_hi / plot.f_lo).powf(t)
            };
            (hz, curve_at(&pts, spline, hz))
        })
        .collect();
    let v = Arc::new(v);
    ui.ctx().data_mut(|d| d.insert_temp(id, (h, v.clone())));
    v
}

fn hz_text(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

pub fn panel(ui: &mut egui::Ui, fx: &mut Fx, env: &Env) {
    let th = &env.th;
    let scale_id = egui::Id::new("af2-phase-linear");
    let mut linear: bool = ui
        .ctx()
        .data_mut(|d| *d.get_persisted_mut_or(scale_id, false));
    let limit = if fx.get("range").round() as i64 == 1 {
        360.0
    } else {
        180.0
    };
    let spline = fx.on("spline");
    let size = vec2(520.0, 300.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let plot = Plot {
        rect: egui::Rect::from_min_max(rect.min + vec2(4.0, 6.0), rect.max - vec2(40.0, 16.0)),
        f_lo: 20.0,
        f_hi: 20_000.0,
        linear,
        y_lo: -limit,
        y_hi: limit,
    };
    let pts = points(fx, limit);
    let at = |p: &(usize, f64, f64)| pos2(plot.x(p.1), plot.y(p.2));
    let pointer = resp.hover_pos();
    let near = |pos: Pos2| {
        pts.iter()
            .filter(|p| at(p).distance(pos) <= 9.0)
            .min_by(|a, b| at(a).distance(pos).total_cmp(&at(b).distance(pos)))
            .map(|p| p.0)
    };
    let active_id = fx.uid.with("phase-active");
    let mut active: Option<usize> = ui.ctx().data(|d| d.get_temp(active_id));
    let round = |v: f64| (v * 10.0).round() / 10.0;
    if resp.drag_started() {
        active = resp.interact_pointer_pos().and_then(near);
    }
    if let (Some(i), true) = (active, resp.dragged()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let (hz, deg) = (round(plot.hz(pos.x)), round(plot.value(pos.y)));
            fx.set_live(FREQ_KEYS[i], hz.max(10.0));
            fx.set_live(PHASE_KEYS[i], deg);
        }
    }
    if resp.drag_stopped() {
        if active.is_some() {
            fx.released = true;
        }
        active = None;
    }
    let remove = |fx: &mut Fx, i: usize| {
        let n = pts.len();
        if n <= 2 || i >= n {
            return;
        }
        let last = n - 1;
        if i != last {
            let (f, ph) = (fx.get(FREQ_KEYS[last]), fx.get(PHASE_KEYS[last]));
            fx.set(FREQ_KEYS[i], f);
            fx.set(PHASE_KEYS[i], ph);
        }
        fx.set("points", (n - 1) as f64);
    };
    if resp.secondary_clicked() || resp.double_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(near) {
            remove(fx, i);
        }
    } else if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let n = pts.len();
            if near(pos).is_none() && plot.rect.contains(pos) && n < MAX_POINTS {
                fx.set(FREQ_KEYS[n], round(plot.hz(pos.x)).max(10.0));
                fx.set(PHASE_KEYS[n], round(plot.value(pos.y)));
                fx.set("points", (n + 1) as f64);
            }
        }
    }
    ui.ctx().data_mut(|d| match active {
        Some(a) => d.insert_temp(active_id, a),
        None => d.remove::<usize>(active_id),
    });
    // Drawing (with the values as they are now).
    let p = ui.painter_at(rect.expand(2.0));
    graph::backdrop(&p, plot.rect, th);
    let step = limit / 3.0;
    let ticks: Vec<f64> = (-3..=3).map(|k| (k as f64 * step).round()).collect();
    graph::grid(&p, &plot, th, &ticks, "°");
    let curve = sampled(ui, fx, &plot, spline, limit);
    let line: Vec<Pos2> = curve
        .iter()
        .map(|(hz, deg)| pos2(plot.x(*hz), plot.y(*deg)))
        .collect();
    graph::curve(&p, &plot, &line, th, false);
    let pts = points(fx, limit);
    let hot = active.or_else(|| pointer.and_then(near));
    for q in &pts {
        graph::handle(&p, at(q), th, hot == Some(q.0), None);
    }
    if let Some(q) = hot.and_then(|h| pts.iter().find(|q| q.0 == h)) {
        let text = format!(
            "{} · {}{:.1}°",
            hz_text(q.1),
            if q.2 > 0.0 { "+" } else { "" },
            q.2
        );
        graph::tag(&p, at(q), &text, th, plot.rect);
    }
    if hot.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    // Below the graph.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "Click to add a point (up to {MAX_POINTS}), drag to move it, right-click to remove it."
            ))
            .size(10.5)
            .color(c(th.text_dim)),
        );
    });
    ui.horizontal(|ui| {
        fx.toggle(ui, "spline", "Spline Curves");
        if crate::widgets::button(ui, "Reset", false)
            .on_hover_text("A flat curve at 0°")
            .clicked()
        {
            fx.set("points", 2.0);
            fx.set(FREQ_KEYS[0], 20.0);
            fx.set(PHASE_KEYS[0], 0.0);
            fx.set(FREQ_KEYS[1], 20_000.0);
            fx.set(PHASE_KEYS[1], 0.0);
            for i in 2..MAX_POINTS {
                fx.set(FREQ_KEYS[i], SPREAD[i]);
                fx.set(PHASE_KEYS[i], 0.0);
            }
        }
    });
    egui::Grid::new(fx.uid.with("phase-form"))
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Frequency scale").color(c(th.text_dim)));
            let mut s = usize::from(!linear);
            if crate::widgets::segmented(ui, &mut s, &["Linear", "Logarithmic"], &[]).changed() {
                linear = s == 0;
                ui.ctx().data_mut(|d| d.insert_persisted(scale_id, linear));
            }
            ui.end_row();
            ui.label(RichText::new("Range").color(c(th.text_dim)));
            fx.choice(ui, "range", &[]);
            ui.end_row();
            ui.label(RichText::new("Channel").color(c(th.text_dim)));
            fx.choice(ui, "channels", &[]);
            ui.end_row();
            ui.label(RichText::new("FFT size").color(c(th.text_dim)));
            if let Some(ctl) = fx.ctl("fft").cloned() {
                let cur = (fx.get("fft").round().max(0.0) as usize).min(ctl.choices.len() - 1);
                let mut pick = cur;
                egui::ComboBox::from_id_salt(fx.uid.with("fft"))
                    .selected_text(&ctl.choices[cur])
                    .show_ui(ui, |ui| {
                        for (k, n) in ctl.choices.iter().enumerate() {
                            ui.selectable_value(&mut pick, k, n);
                        }
                    });
                if pick != cur {
                    fx.set("fft", pick as f64);
                }
            }
            ui.end_row();
        });
    if env.spatial {
        ui.label(
            RichText::new("On the speaker bed: the same curve on every speaker pair (the LFE stays as it is, in time).")
                .size(10.5)
                .color(c(th.accent_alt)),
        );
    }
}
