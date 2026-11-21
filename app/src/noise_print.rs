// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Noise reduction from a noise print, in the Waveform editor: select a
//! stretch of only background (room tone before anyone speaks), Capture
//! Noise Print, then Reduce Noise with Print on the file or a selection.
//! The print is kept per channel until the next capture.

use af27_dsp::noise::{self, NoisePrint};
use egui::RichText;

use crate::state::State;
use crate::theme::c;

/// The shortest stretch a print is learned from.
pub const MIN_SECS: f64 = 0.25;

pub struct NoisePrintUi {
    /// One print per channel, and the rate it was learned at.
    pub print: Option<(Vec<NoisePrint>, u32, f64)>,
    pub open: bool,
    pub reduction: f32,
    pub sensitivity: f32,
}

impl Default for NoisePrintUi {
    fn default() -> Self {
        Self {
            print: None,
            open: false,
            reduction: 18.0,
            sensitivity: 1.5,
        }
    }
}

/// Learns a print from the selection.
pub fn capture(st: &mut State) {
    let Some(d) = st.doc() else {
        st.error("Open a file in the Waveform editor first.");
        return;
    };
    let Some((a, b)) = d.sel.filter(|s| s.1 - s.0 >= MIN_SECS) else {
        st.toast("Select at least a quarter of a second of only background noise first.");
        return;
    };
    let r = d.frames_of((a, b));
    let seg = d.buf.slice(r);
    let prints: Vec<NoisePrint> = seg.channels.iter().map(|c| noise::learn(c)).collect();
    let rate = d.buf.rate;
    st.noise_print.print = Some((prints, rate, b - a));
    st.toast(format!("Noise print captured from {:.2} s", b - a));
}

/// Reduces the noise in `ch` (one channel per print; extra channels use
/// the first print).
pub fn reduce_all(channels: &mut [Vec<f32>], prints: &[NoisePrint], reduction: f32, sensitivity: f32) {
    for (i, ch) in channels.iter_mut().enumerate() {
        if let Some(p) = prints.get(i).or(prints.first()) {
            noise::reduce(ch, p, reduction as f64, sensitivity as f64);
        }
    }
}

pub fn show(ctx: &egui::Context, st: &mut State) {
    if !st.noise_print.open {
        return;
    }
    let th = st.theme.clone();
    let mut open = true;
    let mut apply = false;
    egui::Window::new("Reduce Noise with Print")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let np = &mut st.noise_print;
            match &np.print {
                Some((_, rate, secs)) => ui.label(
                    RichText::new(format!("Print: {secs:.2} s at {rate} Hz"))
                        .size(11.0)
                        .color(c(th.text_dim)),
                ),
                None => ui.label(
                    RichText::new("No print yet: select only background noise and use Effects > Capture Noise Print.")
                        .size(11.0)
                        .color(c(th.meter_high)),
                ),
            };
            egui::Grid::new("np").num_columns(2).show(ui, |ui| {
                ui.label("Reduce by");
                ui.add(egui::Slider::new(&mut np.reduction, 0.0..=40.0).suffix(" dB"));
                ui.end_row();
                ui.label("Sensitivity");
                ui.add(egui::Slider::new(&mut np.sensitivity, 1.0..=4.0))
                    .on_hover_text("Higher treats more of the sound as noise");
                ui.end_row();
            });
            if ui
                .add_enabled(np.print.is_some(), egui::Button::new("Apply"))
                .clicked()
            {
                apply = true;
            }
        });
    if apply {
        let (red, sens) = (st.noise_print.reduction, st.noise_print.sensitivity);
        let Some((prints, rate, _)) = st.noise_print.print.clone() else {
            return;
        };
        let Some(d) = st.doc_mut() else { return };
        if d.buf.rate != rate {
            st.toast("The print was captured at another sample rate: capture one from this file.");
            return;
        }
        let r = d.target();
        let mut b = (*d.buf).clone();
        let mut seg = b.slice(r);
        reduce_all(&mut seg.channels, &prints, red, sens);
        let seg = d.chans.fit_len(seg, r.end - r.start);
        b.replace(r, &seg);
        d.set_buffer_on("Noise Reduction (print)", b);
        open = false;
    }
    st.noise_print.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_from_one_channel_serves_the_rest() {
        let mut s = 9u32;
        let mut rng = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let hiss: Vec<f32> = (0..48_000).map(|_| rng() * 0.02).collect();
        let prints = vec![noise::learn(&hiss)];
        let mut chans = vec![hiss.clone(), hiss.clone(), hiss.clone()];
        reduce_all(&mut chans, &prints, 20.0, 1.5);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        for ch in &chans {
            assert!(rms(&ch[4000..44_000]) < rms(&hiss[4000..44_000]) * 0.3);
        }
    }
}
