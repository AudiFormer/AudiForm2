// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The Properties panel: the selected clip's facts and settings. Stretch
//! and Pitch (mode, type, duration, stretch %, semitones, and the
//! advanced analysis settings), clip gain, colour, reverse, polarity,
//! lock and mute.

use egui::{vec2, RichText};

use af27_core::{StretchKind, StretchMode};

use crate::state::State;
use crate::theme::{c, Theme};

/// Seconds as m:ss.mmm.
fn clock(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor();
    format!("{}:{:06.3}", m as u64, t - m * 60.0)
}

fn row(ui: &mut egui::Ui, th: &Theme, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(96.0, 18.0),
            egui::Label::new(RichText::new(label).color(c(th.text_dim))),
        );
        add(ui);
    });
}

fn section(ui: &mut egui::Ui, th: &Theme, title: &str, right: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().color(c(th.text)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(right).color(c(th.text_dim)));
        });
    });
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [r.left_top(), egui::pos2(r.right(), r.top())],
        egui::Stroke::new(1.0_f32, c(th.border)),
    );
    ui.add_space(3.0);
}

pub fn panel(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    let Some((ti, ci)) = st.selected_clip.and_then(|id| st.session.find_clip(id)) else {
        ui.add_space(8.0);
        ui.label(RichText::new("Select a clip to see its properties.").color(c(th.text_dim)));
        return;
    };
    let before = st.session.clone();
    let track_name = st.session.tracks[ti].name.clone();
    let clip = st.session.tracks[ti].clips[ci].clone();
    let media = st.session.media(clip.media).cloned();
    let mut c2 = clip.clone();
    let mut live_drag = false;
    let mut done = false;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut c2.name)
                        .desired_width(ui.available_width() - 4.0),
                );
                done |= r.lost_focus();
            });
            ui.label(
                RichText::new(format!("On {track_name}"))
                    .size(10.5)
                    .color(c(th.text_dim)),
            );

            section(ui, &th, "Clip", "");
            let src_len = clip.length / clip.props.stretch.applied_ratio();
            row(ui, &th, "Start", |ui| {
                ui.label(clock(clip.start));
            });
            row(ui, &th, "Length", |ui| {
                ui.label(clock(clip.length));
            });
            row(ui, &th, "Natural length", |ui| {
                ui.label(clock(src_len));
            });
            if let Some(m) = &media {
                row(ui, &th, "File", |ui| {
                    ui.add(egui::Label::new(&m.name).truncate()).on_hover_text(
                        m.path
                            .as_ref()
                            .map_or("Not saved yet".into(), |p| p.display().to_string()),
                    );
                });
                row(ui, &th, "Format", |ui| {
                    ui.label(format!(
                        "{} Hz, {}",
                        m.rate,
                        match m.channels {
                            1 => "mono".to_string(),
                            2 => "stereo".to_string(),
                            n => format!("{n} channels"),
                        }
                    ));
                });
            }
            row(ui, &th, "Gain", |ui| {
                let r = crate::track_views::dial(
                    ui,
                    &mut c2.gain_db,
                    -96.0..=24.0,
                    0.0,
                    |v| format!("{v:+.1} dB"),
                    &th,
                );
                live_drag |= r.dragged();
                done |= r.drag_stopped() || (r.changed() && !r.dragged());
            });
            row(ui, &th, "Colour", |ui| {
                let mut own = c2.props.color.is_some();
                if ui.checkbox(&mut own, "Own colour").changed() {
                    c2.props.color = own.then_some(st.session.tracks[ti].color);
                    done = true;
                }
                if let Some(col) = &mut c2.props.color {
                    let mut rgb = egui::Color32::from_rgb(col[0], col[1], col[2]);
                    if ui.color_edit_button_srgba(&mut rgb).changed() {
                        *col = [rgb.r(), rgb.g(), rgb.b()];
                        done = true;
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                done |= ui.checkbox(&mut c2.muted, "Mute").changed();
                done |= ui.checkbox(&mut c2.props.locked, "Lock position").changed();
                done |= ui.checkbox(&mut c2.props.reversed, "Reverse").changed();
                done |= ui
                    .checkbox(&mut c2.props.inverted, "Invert polarity")
                    .changed();
            });

            // Stretch and pitch.
            let s = &mut c2.props.stretch;
            let mode_name = |m: StretchMode| match m {
                StretchMode::Off => "Off",
                StretchMode::Live => "Live",
                StretchMode::Rendered => "Rendered",
            };
            section(ui, &th, "Stretch", mode_name(s.mode));
            row(ui, &th, "Mode", |ui| {
                egui::ComboBox::from_id_salt("clip-stretch-mode")
                    .selected_text(mode_name(s.mode))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for m in [StretchMode::Off, StretchMode::Live, StretchMode::Rendered] {
                            done |= ui.selectable_value(&mut s.mode, m, mode_name(m)).changed();
                        }
                    });
                if ui
                    .small_button("Reset")
                    .on_hover_text("Remove the stretch and pitch")
                    .clicked()
                {
                    *s = Default::default();
                    done = true;
                }
            });
            let on = s.mode != StretchMode::Off;
            ui.add_enabled_ui(on, |ui| {
                row(ui, &th, "Type", |ui| {
                    let kind_name = |k: StretchKind| match k {
                        StretchKind::Monophonic => "Monophonic",
                        StretchKind::Polyphonic => "Polyphonic",
                        StretchKind::Varispeed => "Varispeed",
                    };
                    egui::ComboBox::from_id_salt("clip-stretch-kind")
                        .selected_text(kind_name(s.kind))
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for (k, tip) in [
                                (StretchKind::Monophonic, "One voice or instrument at a time"),
                                (StretchKind::Polyphonic, "Chords and full mixes"),
                                (
                                    StretchKind::Varispeed,
                                    "Tape-style: speed and pitch together",
                                ),
                            ] {
                                done |= ui
                                    .selectable_value(&mut s.kind, k, kind_name(k))
                                    .on_hover_text(tip)
                                    .changed();
                            }
                        });
                });
                row(ui, &th, "Duration", |ui| {
                    let mut dur = src_len * s.ratio;
                    let r = ui.add(
                        egui::DragValue::new(&mut dur)
                            .range(src_len * 0.25..=src_len * 4.0)
                            .speed(0.01)
                            .custom_formatter(|v, _| clock(v))
                            .custom_parser(|t| crate::transport::parse_plain(t)),
                    );
                    if r.changed() && src_len > 0.0 {
                        s.ratio = (dur / src_len).clamp(0.25, 4.0);
                    }
                    live_drag |= r.dragged();
                    done |= r.drag_stopped() || (r.changed() && !r.dragged());
                });
                row(ui, &th, "Stretch", |ui| {
                    let mut pct = (s.ratio * 100.0) as f32;
                    let r =
                        ui.add(crate::widgets::Slider::new(&mut pct, 25.0..=400.0).suffix(" %"));
                    if r.changed() {
                        s.ratio = (pct as f64 / 100.0).clamp(0.25, 4.0);
                    }
                    live_drag |= r.dragged();
                    done |= r.drag_stopped() || (r.changed() && !r.dragged());
                });
                row(ui, &th, "Pitch", |ui| {
                    if s.kind == StretchKind::Varispeed {
                        ui.label(format!(
                            "{:+.2} semitones (follows the speed)",
                            -12.0 * s.ratio.log2()
                        ));
                    } else {
                        let r = ui.add(
                            crate::widgets::Slider::new(&mut s.pitch, -24.0..=24.0)
                                .step_by(0.1)
                                .suffix(" st"),
                        );
                        live_drag |= r.dragged();
                        done |= r.drag_stopped() || (r.changed() && !r.dragged());
                    }
                });
                if s.kind != StretchKind::Varispeed {
                    ui.horizontal(|ui| {
                        ui.add_space(100.0);
                        if ui.small_button("−1 st").clicked() {
                            s.pitch = (s.pitch - 1.0).max(-24.0);
                            done = true;
                        }
                        if ui.small_button("+1 st").clicked() {
                            s.pitch = (s.pitch + 1.0).min(24.0);
                            done = true;
                        }
                        if ui.small_button("Reset").clicked() {
                            s.pitch = 0.0;
                            s.ratio = 1.0;
                            done = true;
                        }
                    });
                }
                ui.collapsing("Advanced", |ui| {
                    row(ui, &th, "Precision", |ui| {
                        egui::ComboBox::from_id_salt("clip-precision")
                            .selected_text(if s.high_precision { "High" } else { "Medium" })
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                done |= ui
                                    .selectable_value(&mut s.high_precision, true, "High")
                                    .changed();
                                done |= ui
                                    .selectable_value(&mut s.high_precision, false, "Medium")
                                    .changed();
                            });
                    });
                    row(ui, &th, "Transients", |ui| {
                        let r = ui.add(
                            crate::widgets::Slider::new(&mut s.transients, 0.0..=100.0)
                                .suffix(" %"),
                        );
                        done |= r.drag_stopped() || (r.changed() && !r.dragged());
                        live_drag |= r.dragged();
                    });
                    row(ui, &th, "Window size", |ui| {
                        let r = ui.add(
                            crate::widgets::Slider::new(&mut s.window_ms, 5.0..=200.0)
                                .suffix(" ms"),
                        );
                        done |= r.drag_stopped() || (r.changed() && !r.dragged());
                        live_drag |= r.dragged();
                    });
                    done |= ui
                        .checkbox(&mut s.preserve_formants, "Preserve formants")
                        .changed();
                });
            });
        });

    if c2 != clip {
        // The length follows the stretch: the same source, at the new speed.
        let new_ratio = c2.props.stretch.applied_ratio();
        let old_ratio = clip.props.stretch.applied_ratio();
        if (new_ratio - old_ratio).abs() > 1e-12 {
            c2.length = clip.length / old_ratio * new_ratio;
            c2.fade_in = c2.fade_in.min(c2.length);
            c2.fade_out = c2.fade_out.min(c2.length);
        }
        st.session.tracks[ti].clips[ci] = c2;
        st.session_version += 1;
        if live_drag && !done {
            if st.gesture_before.is_none() {
                st.gesture_before = Some(before);
            }
        } else {
            let b = st.gesture_before.take().unwrap_or(before);
            st.commit_session("Clip properties", b);
        }
    } else if done {
        if let Some(b) = st.gesture_before.take() {
            if b != st.session {
                st.commit_session("Clip properties", b);
            }
        }
    }
}
