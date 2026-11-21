// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Properties panel: the selected clip in folding sections. Info
//! (start, end and duration, which can be typed or dragged, and the
//! source file, its length, format and path), Basic Settings (clip gain,
//! colour, lock, mute, reverse, polarity), Stretch (mode, type, duration,
//! stretch %, pitch, and the advanced analysis settings) and Source
//! Channel Routing ([`source_channels`]: a grid of which file channel
//! goes to the clip's left and right, with levels). For a MIDI clip
//! ([`midi_panel`]): its place, timing
//! (follow the tempo or keep its speed), stretch, pitch and notes.

use egui::{vec2, RichText};

use af27_core::session::{usual_channel_map, ChannelSend};
use af27_core::{StretchKind, StretchMode};

use crate::state::State;
use crate::theme::{c, Theme};

/// Seconds as m:ss.mmm.
fn clock(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor();
    format!("{}:{:06.3}", m as u64, t - m * 60.0)
}

/// Label column width.
const LABEL_W: f32 = 118.0;

fn row(ui: &mut egui::Ui, th: &Theme, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(LABEL_W, 18.0),
            egui::Label::new(RichText::new(label).color(c(th.text_dim))),
        );
        // Sliders fit what is left of the row, with room for the value.
        ui.spacing_mut().slider_width = (ui.available_width() - 110.0).max(50.0);
        add(ui);
    });
}

/// A combo box's width in what is left of a row (`spare` kept free).
fn combo_w(ui: &egui::Ui, spare: f32) -> f32 {
    (ui.available_width() - spare).clamp(70.0, 150.0)
}

/// A fact, read only.
fn info_row(ui: &mut egui::Ui, th: &Theme, label: &str, text: &str) {
    row(ui, th, label, |ui| {
        ui.add(egui::Label::new(RichText::new(text).color(c(th.text))).truncate());
    });
}

/// A time that can be dragged or typed, in the accent colour. True when
/// it changed.
fn time_row(
    ui: &mut egui::Ui,
    th: &Theme,
    label: &str,
    t: &mut f64,
    live_drag: &mut bool,
    done: &mut bool,
) -> bool {
    let mut changed = false;
    row(ui, th, label, |ui| {
        ui.visuals_mut().override_text_color = Some(c(th.accent));
        let r = ui.add(
            egui::DragValue::new(t)
                .speed(0.005)
                .range(0.0..=f64::MAX)
                .custom_formatter(|v, _| clock(v))
                .custom_parser(|s| crate::transport::parse_plain(s)),
        );
        changed = r.changed();
        *live_drag |= r.dragged();
        *done |= r.drag_stopped() || (r.changed() && !r.dragged());
    });
    changed
}

/// A section that folds, with a summary on the right of its header.
fn fold(
    ui: &mut egui::Ui,
    th: &Theme,
    id: &str,
    title: &str,
    right: &str,
    open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    // For shots: AF2_PROPS_OPEN=stretch,routing starts just those open.
    let open = match std::env::var("AF2_PROPS_OPEN") {
        Ok(v) => v.split(',').any(|k| id.ends_with(k.trim())),
        Err(_) => open,
    };
    let id = ui.make_persistent_id(id);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [r.left_top(), egui::pos2(r.right(), r.top())],
        egui::Stroke::new(1.0_f32, c(th.border)),
    );
    state
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).color(c(th.text)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(right).color(c(th.text_dim)));
            });
        })
        .body(|ui| {
            body(ui);
            ui.add_space(4.0);
        });
}

/// A small waveform mark, by the clip's name.
fn wave_mark(p: &egui::Painter, r: egui::Rect, col: egui::Color32) {
    let hs = [0.3f32, 0.7, 1.0, 0.55, 0.85, 0.4, 0.2];
    let step = r.width() / hs.len() as f32;
    for (i, h) in hs.iter().enumerate() {
        let x = r.left() + step * (i as f32 + 0.5);
        let half = r.height() * 0.45 * h;
        p.line_segment(
            [egui::pos2(x, r.center().y - half), egui::pos2(x, r.center().y + half)],
            egui::Stroke::new(1.3_f32, col),
        );
    }
}

/// A long path with its middle cut out, keeping both ends.
fn middle_cut(s: &str, most: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= most {
        return s.to_string();
    }
    let head = most / 2 - 1;
    let tail = most - head - 1;
    let a: String = chars[..head].iter().collect();
    let b: String = chars[chars.len() - tail..].iter().collect();
    format!("{a}…{b}")
}

/// A file's format words, read once per file.
fn format_of(path: &std::path::Path) -> String {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(s) = cache.lock().ok().and_then(|m| m.get(path).cloned()) {
        return s;
    }
    let s = af27_io::meta::describe_format(path);
    if let Ok(mut m) = cache.lock() {
        m.insert(path.to_path_buf(), s.clone());
    }
    s
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
        if let Some((ti, ci)) = st.selected_clip.and_then(|id| st.session.find_midi(id)) {
            return midi_panel(ui, st, ti, ci);
        }
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
            // The name, by a small wave mark.
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                wave_mark(ui.painter(), r, c(th.text_dim));
                let w = ui.available_width() - 4.0;
                let r = ui.add(egui::TextEdit::singleline(&mut c2.name).desired_width(w));
                done |= r.lost_focus();
            });
            ui.label(
                RichText::new(format!("On {track_name}"))
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
            ui.add_space(4.0);

            let src_len = clip.length / clip.props.stretch.applied_ratio();

            // ---- Info
            fold(ui, &th, "clip-props-info", "Info", "", true, |ui| {
                let mut start = c2.start;
                if time_row(ui, &th, "Clip Start Time", &mut start, &mut live_drag, &mut done) {
                    c2.start = start.max(0.0);
                }
                let mut end = c2.start + c2.length;
                if time_row(ui, &th, "Clip End Time", &mut end, &mut live_drag, &mut done) {
                    // The end trims the clip, within what its file holds.
                    let most = media
                        .as_ref()
                        .map_or(f64::MAX, |m| (m.duration - c2.offset).max(0.0))
                        * c2.props.stretch.applied_ratio();
                    c2.length = (end - c2.start).clamp(0.001, most.max(0.001));
                }
                let mut len = c2.length;
                if time_row(ui, &th, "Clip Duration", &mut len, &mut live_drag, &mut done) {
                    let most = media
                        .as_ref()
                        .map_or(f64::MAX, |m| (m.duration - c2.offset).max(0.0))
                        * c2.props.stretch.applied_ratio();
                    c2.length = len.clamp(0.001, most.max(0.001));
                }
                ui.add_space(6.0);
                match &media {
                    Some(m) => {
                        info_row(ui, &th, "Source File", &m.name);
                        info_row(ui, &th, "Duration", &clock(m.duration));
                        let fmt = m
                            .path
                            .as_deref()
                            .map_or_else(|| "Not saved yet".to_string(), format_of);
                        let chans = match m.channels {
                            1 => "mono".to_string(),
                            2 => "stereo".to_string(),
                            n => format!("{n} channels"),
                        };
                        info_row(ui, &th, "Format", &fmt);
                        info_row(ui, &th, "Sample Rate", &format!("{} Hz, {chans}", m.rate));
                        let path = m
                            .path
                            .as_ref()
                            .map_or("Not saved yet".into(), |p| p.display().to_string());
                        row(ui, &th, "File Path", |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(middle_cut(&path, 34)).color(c(th.text)))
                                    .truncate(),
                            )
                            .on_hover_text(&path);
                        });
                    }
                    None => info_row(ui, &th, "Source File", "Missing"),
                }
            });

            // ---- Basic settings
            fold(ui, &th, "clip-props-basic", "Basic Settings", "", true, |ui| {
                row(ui, &th, "Clip Gain", |ui| {
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
                row(ui, &th, "Clip Color", |ui| {
                    let size = vec2(16.0, 16.0);
                    match &mut c2.props.color {
                        Some(col) => {
                            let mut rgb = egui::Color32::from_rgb(col[0], col[1], col[2]);
                            if ui.color_edit_button_srgba(&mut rgb).changed() {
                                *col = [rgb.r(), rgb.g(), rgb.b()];
                                done = true;
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text("Use the track's colour")
                                .clicked()
                            {
                                c2.props.color = None;
                                done = true;
                            }
                        }
                        None => {
                            // No colour of its own: a crossed swatch.
                            let (r, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                            let p = ui.painter();
                            p.rect_stroke(r, 2.0, egui::Stroke::new(1.0_f32, c(th.text_dim)));
                            p.line_segment(
                                [r.left_bottom(), r.right_top()],
                                egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(220, 60, 60)),
                            );
                            if resp
                                .on_hover_text("The track's colour. Click to give the clip its own")
                                .clicked()
                            {
                                c2.props.color = Some(st.session.tracks[ti].color);
                                done = true;
                            }
                        }
                    }
                });
                ui.add_space(2.0);
                done |= ui.checkbox(&mut c2.props.locked, "Lock in Time").changed();
                ui.add_enabled(false, egui::Checkbox::new(&mut false, "Loop"))
                    .on_disabled_hover_text("Looping clips are coming: a clip that repeats its audio as you stretch its edge");
                done |= ui.checkbox(&mut c2.muted, "Mute").changed();
                done |= ui.checkbox(&mut c2.props.reversed, "Reverse").changed();
                done |= ui
                    .checkbox(&mut c2.props.inverted, "Invert Polarity")
                    .changed();
            });

            // ---- Stretch
            let s = &mut c2.props.stretch;
            let mode_name = |m: StretchMode| match m {
                StretchMode::Off => "Off",
                StretchMode::Live => "Live",
                StretchMode::Rendered => "Rendered",
            };
            let summary = if s.mode == StretchMode::Off {
                "Off".to_string()
            } else {
                format!("{:.0}%  ({})", s.ratio * 100.0, mode_name(s.mode))
            };
            fold(ui, &th, "clip-props-stretch", "Stretch", &summary, false, |ui| {
                row(ui, &th, "Mode", |ui| {
                    egui::ComboBox::from_id_salt("clip-stretch-mode")
                        .selected_text(mode_name(s.mode))
                        .width(combo_w(ui, 36.0))
                        .show_ui(ui, |ui| {
                            for (m, tip) in [
                                (StretchMode::Off, "Plays at its own speed and pitch"),
                                (StretchMode::Live, "Worked out while playing"),
                                (StretchMode::Rendered, "Worked out once and kept"),
                            ] {
                                done |= ui
                                    .selectable_value(&mut s.mode, m, mode_name(m))
                                    .on_hover_text(tip)
                                    .changed();
                            }
                        });
                    if ui
                        .button("↺")
                        .on_hover_text("Reset the stretch and pitch")
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
                            .width(combo_w(ui, 10.0))
                            .show_ui(ui, |ui| {
                                for (k, tip) in [
                                    (StretchKind::Monophonic, "One voice or instrument at a time"),
                                    (StretchKind::Polyphonic, "Chords and full mixes"),
                                    (StretchKind::Varispeed, "Tape-style: speed and pitch together"),
                                ] {
                                    done |= ui
                                        .selectable_value(&mut s.kind, k, kind_name(k))
                                        .on_hover_text(tip)
                                        .changed();
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            vec2(LABEL_W, 18.0),
                            egui::Label::new(RichText::new("Duration").color(c(th.text_dim))),
                        );
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
                        let mut pct = s.ratio * 100.0;
                        let r = ui.add(
                            egui::DragValue::new(&mut pct)
                                .range(25.0..=400.0)
                                .speed(0.5)
                                .max_decimals(1)
                                .suffix(" %"),
                        );
                        if r.changed() {
                            s.ratio = (pct / 100.0).clamp(0.25, 4.0);
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
                            if r.double_clicked() {
                                s.pitch = 0.0;
                                done = true;
                            }
                            live_drag |= r.dragged();
                            done |= r.drag_stopped() || (r.changed() && !r.dragged());
                        }
                    });
                    egui::CollapsingHeader::new("Advanced")
                        .id_salt("clip-props-advanced")
                        .default_open(true)
                        .show(ui, |ui| {
                            row(ui, &th, "Precision", |ui| {
                                egui::ComboBox::from_id_salt("clip-precision")
                                    .selected_text(if s.high_precision { "High" } else { "Medium" })
                                    .width(combo_w(ui, 10.0))
                                    .show_ui(ui, |ui| {
                                        done |= ui
                                            .selectable_value(&mut s.high_precision, true, "High")
                                            .changed();
                                        done |= ui
                                            .selectable_value(&mut s.high_precision, false, "Medium")
                                            .changed();
                                    });
                            });
                            row(ui, &th, "Transient Sensitivity", |ui| {
                                let r = ui.add(
                                    crate::widgets::Slider::new(&mut s.transients, 0.0..=100.0)
                                        .suffix(" %"),
                                );
                                done |= r.drag_stopped() || (r.changed() && !r.dragged());
                                live_drag |= r.dragged();
                            });
                            row(ui, &th, "Window Size", |ui| {
                                let r = ui.add(
                                    crate::widgets::Slider::new(&mut s.window_ms, 5.0..=200.0)
                                        .suffix(" ms"),
                                );
                                done |= r.drag_stopped() || (r.changed() && !r.dragged());
                                live_drag |= r.dragged();
                            });
                            done |= ui
                                .checkbox(&mut s.preserve_formants, "Preserve Formants")
                                .changed();
                        });
                });
            });

            // ---- Source channel routing
            let channels = media.as_ref().map_or(0, |m| m.channels as usize);
            let routed = if c2.props.channel_map.is_some() { "Custom" } else { "" };
            fold(ui, &th, "clip-props-routing", "Source Channel Routing", routed, false, |ui| {
                source_channels(ui, &th, &mut c2, channels, &mut live_drag, &mut done);
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
            // The clip's lanes stretch with it.
            c2.scale_lanes(new_ratio / old_ratio);
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

/// Source Channel Routing: a grid with a row per channel of the clip's
/// file (`channels`, 0 when not known yet) and a column for the clip's
/// left and right. A filled dot sends that channel there; click a dot to
/// switch it. Levels: the amount (-100 to 100 %, below zero flips it)
/// and a polarity switch per channel.
fn source_channels(
    ui: &mut egui::Ui,
    th: &Theme,
    c2: &mut af27_core::Clip,
    channels: usize,
    live_drag: &mut bool,
    done: &mut bool,
) {
    let n = match (&c2.props.channel_map, channels) {
        (_, k) if k > 0 => k,
        (Some(m), _) if !m.is_empty() => m.len(),
        _ => 2,
    };
    // What plays now: the clip's own map, or the file's usual routing.
    let mut rows = c2
        .props
        .channel_map
        .clone()
        .unwrap_or_else(|| usual_channel_map(n));
    rows.resize(
        n,
        ChannelSend {
            left: 0.0,
            right: 0.0,
        },
    );
    let name = |k: usize| match (n, k) {
        (1, _) => "1: Mono".to_string(),
        (2, 0) => "1: Left".to_string(),
        (2, 1) => "2: Right".to_string(),
        _ => format!("{}: Channel {}", k + 1, k + 1),
    };
    ui.label(RichText::new("File to Clip Routing:").color(c(th.text_dim)));
    let mut edited = false;
    egui::Frame::none()
        .stroke(egui::Stroke::new(1.0_f32, c(th.border)))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            egui::Grid::new("clip-routing-grid")
                .num_columns(3)
                .spacing(vec2(14.0, 6.0))
                .show(ui, |ui| {
                    ui.label("");
                    for h in ["L", "R"] {
                        ui.label(RichText::new(h).strong().color(c(th.text)));
                    }
                    ui.end_row();
                    for (k, s) in rows.iter_mut().enumerate() {
                        ui.label(RichText::new(name(k)).color(c(th.text)));
                        for side in 0..2 {
                            let v = if side == 0 { &mut s.left } else { &mut s.right };
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
                            let p = ui.painter();
                            let on = *v != 0.0;
                            let col = if resp.hovered() { c(th.accent) } else { c(th.text) };
                            if on {
                                p.circle_filled(r.center(), 6.0, col);
                            } else {
                                p.circle_stroke(r.center(), 6.0, egui::Stroke::new(1.5_f32, col));
                            }
                            let tip = format!(
                                "{} to the clip's {}",
                                name(k),
                                if side == 0 { "left" } else { "right" }
                            );
                            if resp.on_hover_text(tip).clicked() {
                                *v = if on { 0.0 } else { 1.0 };
                                edited = true;
                                *done = true;
                            }
                        }
                        ui.end_row();
                    }
                });
        });
    egui::CollapsingHeader::new("Levels")
        .id_salt("clip-routing-levels")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("clip-source-channels")
                .num_columns(4)
                .spacing(vec2(10.0, 4.0))
                .show(ui, |ui| {
                    for h in ["Channel", "To left", "To right", "Polarity"] {
                        ui.label(RichText::new(h).size(10.5).color(c(th.text_dim)));
                    }
                    ui.end_row();
                    for (k, s) in rows.iter_mut().enumerate() {
                        ui.label(name(k));
                        for v in [&mut s.left, &mut s.right] {
                            let mut pct = *v * 100.0;
                            let r = ui.add(
                                egui::DragValue::new(&mut pct)
                                    .range(-100.0..=100.0)
                                    .speed(0.5)
                                    .suffix(" %"),
                            );
                            if r.changed() {
                                *v = (pct / 100.0).clamp(-1.0, 1.0);
                                edited = true;
                            }
                            *live_drag |= r.dragged();
                            *done |= r.drag_stopped() || (r.changed() && !r.dragged());
                        }
                        let mut flip = s.flipped();
                        if ui
                            .checkbox(&mut flip, "Ø")
                            .on_hover_text("Flip this channel's polarity")
                            .changed()
                        {
                            s.left = -s.left;
                            s.right = -s.right;
                            edited = true;
                            *done = true;
                        }
                        ui.end_row();
                    }
                });
        });
    if edited {
        // Back to the usual routing: no map of its own.
        let usual = usual_channel_map(n);
        if rows == usual {
            c2.props.channel_map = None;
            c2.props.fold_channels = false;
        } else {
            c2.props.channel_map = Some(rows);
            c2.props.fold_channels = true;
        }
    }
    if c2.props.channel_map.is_some() && ui.small_button("Usual routing").clicked() {
        c2.props.channel_map = None;
        c2.props.fold_channels = false;
        *done = true;
    }
}

/// The Properties panel for the MIDI clip at track `ti`, clip `ci`.
fn midi_panel(ui: &mut egui::Ui, st: &mut State, ti: usize, ci: usize) {
    use af27_core::midi::{ClipTiming, MIDI_PITCH_MAX, MIDI_STRETCH_MAX, MIDI_STRETCH_MIN};
    let th = st.theme.clone();
    let before = st.session.clone();
    let tempo = st.session.tempo.clone();
    let track_name = st.session.tracks[ti].name.clone();
    let clip = st.session.tracks[ti].midi[ci].clone();
    let mut c2 = clip.clone();
    let mut keep = !clip.timing.is_follow();
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
                RichText::new(format!("MIDI clip on {track_name}"))
                    .size(10.5)
                    .color(c(th.text_dim)),
            );

            section(ui, &th, "Clip", "");
            let start_s = tempo.secs(clip.start);
            let end_s = clip.end_secs(&tempo);
            row(ui, &th, "Start", |ui| {
                ui.label(format!("{}  ({})", tempo.format(start_s), clock(start_s)));
            });
            row(ui, &th, "Length", |ui| {
                let beats = clip.end_beats(&tempo) - clip.start;
                ui.label(format!("{beats:.2} beats  ({})", clock(end_s - start_s)))
                    .on_hover_text(format!(
                        "Its own length: {:.2} beats before stretch and timing",
                        clip.length
                    ));
            });
            row(ui, &th, "Notes", |ui| {
                ui.label(format!("{}", clip.notes.len()));
            });
            ui.horizontal_wrapped(|ui| {
                done |= ui.checkbox(&mut c2.muted, "Mute").changed();
            });

            section(
                ui,
                &th,
                "Timing",
                if keep { "Keep speed" } else { "Follow tempo" },
            );
            row(ui, &th, "Timing", |ui| {
                egui::ComboBox::from_id_salt("midi-clip-timing")
                    .selected_text(if keep { "Keep speed" } else { "Follow tempo" })
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        done |= ui
                            .selectable_value(&mut keep, false, "Follow tempo")
                            .on_hover_text(
                                "Its notes are in the session's beats: it speeds up and slows down with tempo changes under it",
                            )
                            .changed();
                        done |= ui
                            .selectable_value(&mut keep, true, "Keep speed")
                            .on_hover_text(
                                "It plays at its own tempo wherever it sits, the same length in seconds",
                            )
                            .changed();
                    });
            });
            if let ClipTiming::KeepSpeed { bpm } = clip.timing {
                row(ui, &th, "Own tempo", |ui| {
                    ui.label(format!("{} BPM", (bpm * 100.0).round() / 100.0))
                        .on_hover_text("The tempo where it was when it was set to keep its speed");
                });
            }

            section(ui, &th, "Stretch and Pitch", "");
            row(ui, &th, "Stretch", |ui| {
                let mut pct = (c2.ratio() * 100.0) as f32;
                let r = ui.add(
                    crate::widgets::Slider::new(
                        &mut pct,
                        (MIDI_STRETCH_MIN * 100.0) as f32..=(MIDI_STRETCH_MAX * 100.0) as f32,
                    )
                    .suffix(" %"),
                );
                if r.changed() {
                    c2.stretch = (pct as f64 / 100.0).clamp(MIDI_STRETCH_MIN, MIDI_STRETCH_MAX);
                }
                live_drag |= r.dragged();
                done |= r.drag_stopped() || (r.changed() && !r.dragged());
            });
            row(ui, &th, "Pitch", |ui| {
                let r = ui.add(
                    crate::widgets::Slider::new(&mut c2.pitch, -MIDI_PITCH_MAX..=MIDI_PITCH_MAX)
                        .step_by(0.1)
                        .suffix(" st"),
                )
                .on_hover_text(
                    "Whole semitones transpose the notes; a fraction detunes the instrument",
                );
                live_drag |= r.dragged();
                done |= r.drag_stopped() || (r.changed() && !r.dragged());
            });
            ui.horizontal(|ui| {
                ui.add_space(100.0);
                if ui.small_button("−1 st").clicked() {
                    c2.pitch = (c2.pitch - 1.0).max(-MIDI_PITCH_MAX);
                    done = true;
                }
                if ui.small_button("+1 st").clicked() {
                    c2.pitch = (c2.pitch + 1.0).min(MIDI_PITCH_MAX);
                    done = true;
                }
                if ui
                    .small_button("Reset")
                    .on_hover_text("Remove the stretch and pitch")
                    .clicked()
                {
                    c2.pitch = 0.0;
                    c2.stretch = 1.0;
                    done = true;
                }
            });
        });

    if keep != !c2.timing.is_follow() {
        c2.set_timing(keep, &tempo);
    }
    if c2 != clip {
        st.session.tracks[ti].midi[ci] = c2;
        st.session_version += 1;
        if live_drag && !done {
            if st.gesture_before.is_none() {
                st.gesture_before = Some(before);
            }
        } else {
            let b = st.gesture_before.take().unwrap_or(before);
            st.commit_session("MIDI clip properties", b);
        }
    } else if done {
        if let Some(b) = st.gesture_before.take() {
            if b != st.session {
                st.commit_session("MIDI clip properties", b);
            }
        }
    }
}
