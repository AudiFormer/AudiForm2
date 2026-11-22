// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The Loudness panel: a live meter on what is playing (after the master
//! fader): momentary, short-term and integrated loudness, loudness range
//! and true peak, read against a delivery target, with the last minute of
//! short-term loudness drawn as a line.

use af2_dsp::loudness::LiveLoudness;
use egui::{pos2, vec2, RichText, Stroke};

use crate::state::State;
use crate::theme::c;

/// Delivery targets: (name, LUFS, true-peak ceiling dBTP).
pub const TARGETS: [(&str, f64, f64); 6] = [
    ("Podcast, stereo", -16.0, -1.0),
    ("Podcast, mono", -19.0, -1.0),
    ("Streaming music", -14.0, -1.0),
    ("Broadcast, Europe (EBU R128)", -23.0, -1.0),
    ("Broadcast, US (ATSC A/85)", -24.0, -2.0),
    ("Audiobook", -20.0, -3.0),
];

/// Short-term readings kept for the line (10 a second, one minute).
const HISTORY: usize = 600;

pub struct Meter {
    pub live: LiveLoudness,
    /// Where we are in the output ring.
    pos: usize,
    pub target: usize,
    history: Vec<f32>,
    /// Steps measured, to know when to add to the line.
    last_step: usize,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            live: LiveLoudness::new(48_000),
            pos: af2_engine::scope::written(),
            target: 0,
            history: Vec::new(),
            last_step: 0,
        }
    }
}

impl Meter {
    /// Takes in what played since the last frame.
    pub fn poll(&mut self, playing: bool) {
        let (frames, pos) = af2_engine::scope::since(self.pos);
        self.pos = pos;
        if !playing || frames.is_empty() {
            return;
        }
        let rate = af2_engine::scope::rate();
        if rate != self.live.rate() {
            self.live = LiveLoudness::new(rate);
            self.history.clear();
            self.last_step = 0;
        }
        self.live.feed(&frames);
        let steps = (self.live.measured_secs() * 10.0) as usize;
        if steps > self.last_step {
            self.last_step = steps;
            self.history
                .push(self.live.short_term().unwrap_or(-70.0).max(-70.0) as f32);
            if self.history.len() > HISTORY {
                self.history.drain(..self.history.len() - HISTORY);
            }
        }
    }

    pub fn reset(&mut self) {
        self.live.reset();
        self.history.clear();
        self.last_step = 0;
    }
}

/// How a reading stands against the target: under, on (within 1 LU),
/// over.
pub fn verdict(value: f64, target: f64) -> std::cmp::Ordering {
    if (value - target).abs() <= 1.0 {
        std::cmp::Ordering::Equal
    } else if value < target {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let playing = st.is_playing();
    st.loudness.poll(playing);
    if playing {
        ui.ctx().request_repaint();
    }
    let th = st.theme.clone();
    let m = &mut st.loudness;
    let (name, target, ceiling) = TARGETS[m.target.min(TARGETS.len() - 1)];

    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("loud-target")
            .selected_text(format!("{name}: {target} LUFS"))
            .show_ui(ui, |ui| {
                for (i, t) in TARGETS.iter().enumerate() {
                    ui.selectable_value(&mut m.target, i, format!("{}: {} LUFS, {} dBTP", t.0, t.1, t.2));
                }
            });
        if ui.button("Reset").on_hover_text("Start the integrated reading, range and peak again").clicked() {
            m.reset();
        }
    });

    let colour = |v: Option<f64>| match v.map(|v| verdict(v, target)) {
        Some(std::cmp::Ordering::Equal) => c(th.meter_low),
        Some(std::cmp::Ordering::Greater) => c(th.meter_high),
        _ => c(th.text),
    };
    let show = |v: Option<f64>| v.map_or("—".to_string(), |v| format!("{v:.1}"));
    let live = &m.live;
    let (mo, sh, int) = (live.momentary(), live.short_term(), live.integrated());

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("INTEGRATED").size(10.0).color(c(th.text_dim)));
            ui.label(RichText::new(show(int)).size(30.0).monospace().color(colour(int)));
            ui.label(RichText::new("LUFS").size(10.0).color(c(th.text_dim)));
        });
        ui.add_space(12.0);
        egui::Grid::new("loud-grid").num_columns(2).show(ui, |ui| {
            let mut row = |k: &str, v: String, col: egui::Color32| {
                ui.label(RichText::new(k).size(11.0).color(c(th.text_dim)));
                ui.label(RichText::new(v).monospace().color(col));
                ui.end_row();
            };
            row("Momentary", format!("{} LUFS", show(mo)), colour(mo));
            row("Short-term", format!("{} LUFS", show(sh)), colour(sh));
            row("Range", format!("{} LU", show(live.range())), c(th.text));
            let tp = live.true_peak_db();
            let tp_col = if tp > ceiling { c(th.meter_high) } else { c(th.text) };
            row(
                "True peak",
                if live.max_true_peak > 0.0 { format!("{tp:.1} dBTP") } else { "—".into() },
                tp_col,
            );
            row("Measured", af2_core::time::format_time(live.measured_secs()), c(th.text_dim));
        });
    });

    // The last minute of short-term loudness, with the target band.
    let h = ui.available_height().clamp(60.0, 160.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, c(th.panel_raised));
    let (lo, hi) = (-50.0f32, -5.0f32);
    let y = |v: f32| rect.bottom() - (v.clamp(lo, hi) - lo) / (hi - lo) * rect.height();
    let band = egui::Rect::from_x_y_ranges(rect.x_range(), y(target as f32 + 1.0)..=y(target as f32 - 1.0));
    p.rect_filled(band, 0.0, c(th.meter_low).gamma_multiply(0.18));
    p.line_segment(
        [pos2(rect.left(), y(target as f32)), pos2(rect.right(), y(target as f32))],
        Stroke::new(1.0_f32, c(th.meter_low).gamma_multiply(0.6)),
    );
    if m.history.len() >= 2 {
        let dx = rect.width() / (HISTORY - 1) as f32;
        let x0 = rect.right() - dx * (m.history.len() - 1) as f32;
        let pts: Vec<egui::Pos2> = m
            .history
            .iter()
            .enumerate()
            .map(|(i, v)| pos2(x0 + dx * i as f32, y(*v)))
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, c(th.accent))));
    }
    if !playing && m.history.is_empty() {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Play to measure",
            egui::FontId::proportional(12.0),
            c(th.text_dim),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn readings_within_one_lu_are_on_target() {
        assert_eq!(verdict(-16.4, -16.0), Ordering::Equal);
        assert_eq!(verdict(-18.0, -16.0), Ordering::Less);
        assert_eq!(verdict(-13.9, -16.0), Ordering::Greater);
        for t in TARGETS {
            assert!(t.1 < 0.0 && t.2 <= 0.0, "{}", t.0);
        }
    }
}
