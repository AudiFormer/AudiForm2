// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The built-in synth's window, for the track in `st.synth_editor`: a
//! model picker over three bands of numbered sections, in the order a
//! sound is built: Oscillators, Tone and a live Waveform; Filter, Amp
//! and Envelopes; LFO, Voice and Effects. Every band is the same width
//! and its sections the same height, the dials on one grid. The
//! waveform is the voice's own sound for a held note, rendered off the
//! UI thread whenever a setting changes; the filter's curve and the two
//! envelopes are drawn too. Edits go to the playing synth (and the audition) as
//! they happen; each drag, click or wheel turn is one undo step.

use egui::{pos2, vec2, Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke};

use af2_core::midi::{Instrument, Wave};
use af2_core::synth_model::{
    FilterKind, FilterType, LfoShape, SynthModel, VoiceMode, MAX_UNISON, MAX_VOICES,
};

use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};
use crate::widgets;

/// The dials' size.
const KNOB: f32 = 46.0;
/// A dial's width (`widgets::knob` is never narrower) and the column
/// each dial gets in the grid.
const KW: f32 = 66.0;
const CELL: f32 = 72.0;
/// Every band of sections fills this width, so their edges line up.
const WIDTH: f32 = 1150.0;
/// Between sections and rows; inside each section's frame.
const GAP: f32 = 6.0;
const MARGIN: f32 = 8.0;
/// The oscillators' wave picker column, the row names' column.
const PICK: f32 = 262.0;
const LABEL_W: f32 = 46.0;
/// A stepped switch's height.
const SEG_H: f32 = 22.0;
/// The filter curve's width, and the gap before a drawing.
const CURVE_W: f32 = 210.0;
const DRAW_GAP: f32 = 10.0;

/// What the window's widgets did this frame.
#[derive(Default)]
struct Edit {
    changed: bool,
    /// A finished change (a click, a wheel turn, a drag let go).
    done: bool,
}

impl Edit {
    fn take(&mut self, r: &egui::Response) {
        if r.changed() {
            self.changed = true;
            if !r.dragged() {
                self.done = true;
            }
        }
        if r.drag_stopped() {
            self.done = true;
        }
    }
}

/// Draws the window while `st.synth_editor` names a track.
pub fn window(ctx: &egui::Context, st: &mut State) {
    let Some(id) = st.synth_editor else {
        return;
    };
    let Some(ti) = st.session.track_index(id) else {
        st.synth_editor = None;
        return;
    };
    let th = st.theme.clone();
    let name = st.session.tracks[ti].name.clone();
    let font = st.session.tracks[ti].soundfont.is_some();
    let old = st.session.tracks[ti].instrument;
    let mut inst = old;
    let mut ed = Edit::default();
    let mut open = true;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "synth_ui.rs", 0)), format!("Synth: {name}"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            if font {
                ui.label(
                    RichText::new(
                        "This track plays a SoundFont. Pick the built-in synth as its instrument to shape it here.",
                    )
                    .color(c(th.text_dim)),
                );
                return;
            }
            body(ui, &th, &mut inst, &mut ed);
        });
    if !open {
        st.synth_editor = None;
    }
    if ed.changed && inst != old {
        let gesture = egui::Id::new("af2-synth-gesture");
        let mine: bool = ctx.data(|d| d.get_temp(gesture)).unwrap_or(false);
        let before = |st: &State| {
            let mut b = st.session.clone();
            b.tracks[ti].instrument = old;
            b
        };
        if !ed.done && !mine {
            // A drag begins: the session as it was is its undo step.
            st.gesture_before = Some(before(st));
            ctx.data_mut(|d| d.insert_temp(gesture, true));
        }
        let b = if ed.done { Some(before(st)) } else { None };
        st.session.tracks[ti].instrument = inst;
        st.live_instrument(ti);
        if let Some(b) = b {
            let b = if mine {
                st.gesture_before.take().unwrap_or(b)
            } else {
                b
            };
            ctx.data_mut(|d| d.insert_temp(gesture, false));
            st.commit_session("Synth", b);
        }
    } else if ed.done {
        // A drag let go (its last frame may change nothing).
        let gesture = egui::Id::new("af2-synth-gesture");
        if ctx.data(|d| d.get_temp(gesture)).unwrap_or(false) {
            ctx.data_mut(|d| d.insert_temp(gesture, false));
            if let Some(b) = st.gesture_before.take() {
                if b != st.session {
                    st.commit_session("Synth", b);
                }
            }
        }
    }
}

fn body(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    ui.spacing_mut().item_spacing = vec2(GAP, GAP);
    model_row(ui, th, inst, ed);
    ui.add_space(2.0);
    let kh = knob_h(ui);

    // The sound, in the order it is built: the source and what it looks like…
    let w_osc = sec_w(PICK, 4);
    let w_tone = sec_w(0.0, 4);
    let w_scope = WIDTH - w_osc - w_tone - 2.0 * GAP;
    band(ui, "src", |ui| {
        section(ui, th, "1", "Oscillators", w_osc, "src", 0, |ui, iw| {
            oscillators(ui, th, inst, ed, iw, kh)
        });
        section(ui, th, "2", "Tone", w_tone, "src", 1, |ui, iw| {
            tone(ui, inst, ed, iw, kh)
        });
        section(ui, th, "", "Waveform", w_scope, "src", 2, |ui, iw| {
            scope(ui, th, inst, vec2(iw, 2.0 * kh + GAP))
        });
    });
    // …then what shapes it, and the envelopes that move it…
    let w_filter = sec_w(DRAW_GAP + CURVE_W, 3);
    let w_amp = sec_w(0.0, 2);
    let w_env = WIDTH - w_filter - w_amp - 2.0 * GAP;
    band(ui, "shape", |ui| {
        section(ui, th, "3", "Filter", w_filter, "shape", 0, |ui, iw| {
            filter(ui, th, inst, ed, iw, kh)
        });
        section(ui, th, "4", "Amp", w_amp, "shape", 1, |ui, iw| {
            amp(ui, th, inst, ed, iw, kh)
        });
        section(ui, th, "5", "Envelopes", w_env, "shape", 2, |ui, iw| {
            envelopes(ui, th, inst, ed, iw, kh)
        });
    });
    // …then the LFO, how notes take voices, and the effect after it all.
    let w_lfo = sec_w(0.0, 6);
    let w_voice = sec_w(0.0, 6);
    let w_fx = WIDTH - w_lfo - w_voice - 2.0 * GAP;
    band(ui, "play", |ui| {
        section(ui, th, "6", "LFO", w_lfo, "play", 0, |ui, iw| {
            lfo(ui, inst, ed, iw, kh)
        });
        section(ui, th, "7", "Voice", w_voice, "play", 1, |ui, iw| {
            voice(ui, inst, ed, iw, kh)
        });
        section(ui, th, "8", "Effects", w_fx, "play", 2, |ui, iw| {
            effects(ui, th, inst, ed, iw, kh)
        });
    });
}

fn model_row(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("MODEL")
                .size(12.0)
                .strong()
                .extra_letter_spacing(2.0)
                .color(c(th.accent)),
        );
        let cur = inst.ext.model;
        let mut pick = None;
        egui::ComboBox::from_id_salt("synth-model")
            .selected_text(RichText::new(cur.name()).strong())
            .width(170.0)
            .show_ui(ui, |ui| {
                for m in SynthModel::ALL {
                    if ui
                        .selectable_label(m == cur, m.name())
                        .on_hover_text(m.blurb())
                        .clicked()
                    {
                        pick = Some(m);
                    }
                }
            });
        if let Some(m) = pick {
            *inst = m.preset();
            ed.changed = true;
            ed.done = true;
        }
        if inst.model_edited() {
            ui.label(RichText::new("edited").italics().color(c(th.text_dim)));
            if ui
                .small_button("Revert")
                .on_hover_text(format!("Back to {} as it comes", cur.name()))
                .clicked()
            {
                *inst = cur.preset();
                ed.changed = true;
                ed.done = true;
            }
        }
        ui.label(
            RichText::new(cur.blurb())
                .size(11.0)
                .italics()
                .color(c(th.text_dim)),
        );
    });
}

/// A section's width for `cols` dial columns after a `lead` column.
fn sec_w(lead: f32, cols: usize) -> f32 {
    2.0 * MARGIN + lead + cols as f32 * CELL
}

/// The dials' height (dial, name and value box), as last drawn.
fn knob_h(ui: &egui::Ui) -> f32 {
    ui.ctx()
        .data(|d| d.get_temp(egui::Id::new("synth-knob-h")))
        .unwrap_or(82.0)
}

/// A row of sections side by side, all as tall as the tallest.
fn band(ui: &mut egui::Ui, key: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        add(ui);
    });
    let ctx = ui.ctx().clone();
    let tall = (0..3)
        .filter_map(|i| ctx.data(|d| d.get_temp::<f32>(egui::Id::new(("synth-sec-h", key, i)))))
        .fold(0.0f32, f32::max);
    let tk = egui::Id::new(("synth-band-h", key));
    let was: f32 = ctx.data(|d| d.get_temp(tk)).unwrap_or(0.0);
    if (tall - was).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(tk, tall));
        ctx.request_repaint();
    }
}

/// A framed section `w` wide with its step number and title; `add` gets
/// the inner width. Padded at the foot to its band's height.
#[allow(clippy::too_many_arguments)]
fn section(
    ui: &mut egui::Ui,
    th: &Theme,
    step: &str,
    title: &str,
    w: f32,
    band: &str,
    idx: usize,
    add: impl FnOnce(&mut egui::Ui, f32),
) {
    let ctx = ui.ctx().clone();
    let target: f32 = ctx
        .data(|d| d.get_temp(egui::Id::new(("synth-band-h", band))))
        .unwrap_or(0.0);
    egui::Frame::none()
        .fill(lerp(th.panel_raised, th.background, 0.35))
        .stroke(Stroke::new(1.0_f32, ca(th.border, 220)))
        .rounding(if th.bevel.is_some() { 0.0 } else { 5.0 })
        .inner_margin(egui::Margin::same(MARGIN))
        .show(ui, |ui| {
            let iw = w - 2.0 * MARGIN;
            ui.set_width(iw);
            // The frame takes its band's left-to-right layout: stack here.
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                let r = ui
                    .vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if !step.is_empty() {
                                ui.label(
                                    RichText::new(step)
                                        .size(11.0)
                                        .strong()
                                        .color(c(th.accent)),
                                );
                            }
                            ui.label(
                                RichText::new(title.to_uppercase())
                                    .size(11.0)
                                    .strong()
                                    .extra_letter_spacing(2.0)
                                    .color(c(th.text_dim)),
                            );
                        });
                        add(ui, iw);
                    })
                    .response
                    .rect;
                let h = r.height();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new(("synth-sec-h", band, idx)), h));
                if target - h > 0.5 {
                    ui.add_space((target - h - GAP).max(0.0));
                }
            });
        });
}

/// A row of cells (no gaps of its own: each cell is its own width).
fn row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        add(ui);
    });
}

/// One cell `w` wide holding a dial, centred.
fn cell(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(w, 10.0),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.set_min_width(w);
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.add_space(((w - KW) / 2.0).max(0.0));
            add(ui);
        },
    );
}

/// A cell as tall as a dial with `add` centred in it (a switch, a label).
fn cell_mid(ui: &mut egui::Ui, w: f32, h: f32, inner_h: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(w, h),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.set_min_size(vec2(w, h));
            ui.add_space(((h - inner_h) / 2.0).max(0.0));
            add(ui);
        },
    );
}

/// A line the height of a stepped switch, holding a dim caption: keeps
/// dials level with those of sections that have a switch above them.
fn caption_line(ui: &mut egui::Ui, th: &Theme, t: &str) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), SEG_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_height(SEG_H);
            ui.label(RichText::new(t).size(10.5).italics().color(c(th.text_dim)));
        },
    );
}

/// A dial for an f32 control.
#[allow(clippy::too_many_arguments)]
fn knob(
    ui: &mut egui::Ui,
    ed: &mut Edit,
    id: &str,
    label: &str,
    v: &mut f32,
    lo: f32,
    hi: f32,
    default: f32,
    log: bool,
    fmt: &dyn Fn(f64) -> String,
) {
    let mut x = f64::from(*v);
    let r = widgets::knob(
        ui,
        egui::Id::new(("synth-knob", id)),
        &mut x,
        f64::from(lo),
        f64::from(hi),
        f64::from(default),
        log,
        KNOB,
        label,
        &[],
        fmt,
    );
    let h = r.rect.height();
    if h > 20.0 && (h - knob_h(ui)).abs() > 0.5 {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("synth-knob-h"), h));
        ui.ctx().request_repaint();
    }
    if r.changed() {
        *v = x as f32;
    }
    ed.take(&r);
}

/// A dial for a whole-number control.
#[allow(clippy::too_many_arguments)]
fn knob_i(
    ui: &mut egui::Ui,
    ed: &mut Edit,
    id: &str,
    label: &str,
    v: &mut i32,
    lo: i32,
    hi: i32,
    default: i32,
    unit: &str,
) {
    let mut x = *v as f32;
    let u = unit.to_string();
    knob(
        ui,
        ed,
        id,
        label,
        &mut x,
        lo as f32,
        hi as f32,
        default as f32,
        false,
        &move |v| {
            let n = v.round() as i64;
            match u.as_str() {
                "" => format!("{n:+}"),
                "#" => format!("{n}"),
                u => format!("{n} {u}"),
            }
        },
    );
    *v = x.round() as i32;
}

fn secs(v: f64) -> String {
    if v < 1.0 {
        format!("{:.0} ms", v * 1000.0)
    } else {
        format!("{v:.2} s")
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn plain(v: f64) -> String {
    format!("{v:.2}")
}

fn hz(v: f64) -> String {
    widgets::fmt_unit(v, "Hz")
}

fn cents(v: f64) -> String {
    format!("{v:+.0} ct")
}

fn oct(v: f64) -> String {
    format!("{v:+.1} oct")
}

fn st_(v: f64) -> String {
    format!("{v:+.2} st")
}

/// A stepped switch over `items`.
fn steps<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    ed: &mut Edit,
    v: &mut T,
    items: &[(T, &str)],
    tips: &[&str],
) {
    let mut cur = items.iter().position(|x| x.0 == *v).unwrap_or(0);
    let names: Vec<&str> = items.iter().map(|x| x.1).collect();
    let r = widgets::segmented(ui, &mut cur, &names, tips);
    if r.changed() {
        *v = items[cur].0;
        ed.changed = true;
        ed.done = true;
    }
}

const WAVES: [(Wave, &str); 5] = [
    (Wave::Sine, "Sine"),
    (Wave::Triangle, "Tri"),
    (Wave::Saw, "Saw"),
    (Wave::Square, "Square"),
    (Wave::Pulse, "Pulse"),
];

/// A row's name in the accent colour, `w` wide.
fn row_label(ui: &mut egui::Ui, th: &Theme, t: &str, w: f32, h: f32) {
    ui.add_sized(
        vec2(w, h),
        egui::Label::new(RichText::new(t).size(11.0).strong().color(c(th.accent))),
    );
}

/// The wave picker's column: the oscillator's name over its waves,
/// centred on the dials' height.
fn wave_pick(ui: &mut egui::Ui, th: &Theme, kh: f32, name: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(PICK, kh),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_size(vec2(PICK, kh));
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add_space(((kh - (14.0 + 4.0 + SEG_H)) / 2.0 - 6.0).max(0.0));
            ui.label(RichText::new(name).size(11.0).strong().color(c(th.accent)));
            add(ui);
        },
    );
}

fn oscillators(
    ui: &mut egui::Ui,
    th: &Theme,
    inst: &mut Instrument,
    ed: &mut Edit,
    iw: f32,
    kh: f32,
) {
    let def = af2_core::synth_model::SynthControls::default();
    let cw = (iw - PICK) / 4.0;
    row(ui, |ui| {
        wave_pick(ui, th, kh, "OSC 1", |ui| {
            steps(ui, ed, &mut inst.wave, &WAVES, &[]);
        });
        let x = &mut inst.ext;
        cell(ui, cw, |ui| {
            let mut o = x.osc1_octave as i32;
            knob_i(ui, ed, "o1oct", "Octave", &mut o, -3, 3, 0, "");
            x.osc1_octave = o as i8;
        });
        cell(ui, cw, |ui| {
            let mut s = x.osc1_semi as i32;
            knob_i(ui, ed, "o1semi", "Semi", &mut s, -12, 12, 0, "");
            x.osc1_semi = s as i8;
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "o1fine",
                "Fine",
                &mut x.osc1_fine,
                -50.0,
                50.0,
                0.0,
                false,
                &cents,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "o1lvl",
                "Level",
                &mut x.osc1_level,
                0.0,
                1.0,
                def.osc1_level,
                false,
                &pct,
            )
        });
    });
    row(ui, |ui| {
        wave_pick(ui, th, kh, "OSC 2", |ui| {
            let mut w = inst.ext.osc2_wave;
            let items: [(Option<Wave>, &str); 6] = [
                (None, "Same"),
                (Some(Wave::Sine), "Sine"),
                (Some(Wave::Triangle), "Tri"),
                (Some(Wave::Saw), "Saw"),
                (Some(Wave::Square), "Square"),
                (Some(Wave::Pulse), "Pulse"),
            ];
            steps(ui, ed, &mut w, &items, &["The same wave as oscillator 1"]);
            inst.ext.osc2_wave = w;
        });
        cell(ui, cw, |ui| {
            let mut o = inst.ext.osc2_octave as i32;
            knob_i(ui, ed, "o2oct", "Octave", &mut o, -3, 3, 0, "");
            inst.ext.osc2_octave = o as i8;
        });
        cell(ui, cw, |ui| {
            let mut s = inst.ext.osc2_semi as i32;
            knob_i(ui, ed, "o2semi", "Semi", &mut s, -12, 12, 0, "");
            inst.ext.osc2_semi = s as i8;
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "o2fine",
                "Detune",
                &mut inst.detune,
                -50.0,
                50.0,
                7.0,
                false,
                &cents,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "o2lvl",
                "Level",
                &mut inst.ext.osc2_level,
                0.0,
                1.0,
                def.osc2_level,
                false,
                &pct,
            )
        });
    });
}

/// What colours the oscillators: pulse width, sub, noise, sync and FM.
fn tone(ui: &mut egui::Ui, inst: &mut Instrument, ed: &mut Edit, iw: f32, kh: f32) {
    let def = af2_core::synth_model::SynthControls::default();
    let cw = iw / 4.0;
    let x = &mut inst.ext;
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(
                ui, ed, "pw", "Pulse W", &mut x.pw, 0.05, 0.95, def.pw, false, &pct,
            )
        });
        cell(ui, cw, |ui| {
            knob(ui, ed, "sub", "Sub", &mut x.sub, 0.0, 1.0, 0.0, false, &pct)
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "noise",
                "Noise",
                &mut x.noise,
                0.0,
                1.0,
                0.0,
                false,
                &pct,
            )
        });
        cell_mid(ui, cw, kh, 20.0, |ui| {
            let r = widgets::switch(ui, &mut x.sync, "Sync")
                .on_hover_text("Oscillator 2 restarts with every cycle of oscillator 1");
            if r.changed() {
                ed.changed = true;
                ed.done = true;
            }
        });
    });
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(ui, ed, "fm", "FM", &mut x.fm, 0.0, 8.0, 0.0, false, &plain)
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "fmr",
                "FM Ratio",
                &mut x.fm_ratio,
                0.5,
                8.0,
                1.0,
                false,
                &|v| format!("{v:.2}x"),
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "fmd",
                "FM Decay",
                &mut x.fm_decay,
                0.02,
                5.0,
                1.0,
                true,
                &secs,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "sweep",
                "Sync Sweep",
                &mut x.osc2_env,
                -24.0,
                24.0,
                0.0,
                false,
                &|v| format!("{v:+.1} st"),
            )
        });
    });
}

fn filter(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit, iw: f32, kh: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        steps(
            ui,
            ed,
            &mut inst.ext.filter_type,
            &[
                (FilterType::LowPass, "Low-pass"),
                (FilterType::HighPass, "High-pass"),
                (FilterType::BandPass, "Band-pass"),
            ],
            &[],
        );
        steps(
            ui,
            ed,
            &mut inst.ext.filter_kind,
            &[
                (FilterKind::Classic, "12 dB"),
                (FilterKind::Ladder, "24 dB Ladder"),
            ],
            &[
                "The original 2-pole state-variable filter",
                "A 4-pole ladder with drive",
            ],
        );
    });
    let cw = (iw - DRAW_GAP - CURVE_W) / 3.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.vertical(|ui| {
            row(ui, |ui| {
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "cut",
                        "Cutoff",
                        &mut inst.cutoff,
                        40.0,
                        18_000.0,
                        6000.0,
                        true,
                        &hz,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "res",
                        "Resonance",
                        &mut inst.resonance,
                        0.0,
                        1.0,
                        0.15,
                        false,
                        &pct,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "drive",
                        "Drive",
                        &mut inst.ext.drive,
                        0.0,
                        1.0,
                        0.0,
                        false,
                        &pct,
                    )
                });
            });
            row(ui, |ui| {
                let x = &mut inst.ext;
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fenv",
                        "Env Amt",
                        &mut x.filter_env,
                        -6.0,
                        6.0,
                        0.0,
                        false,
                        &oct,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "ktrk",
                        "Key Track",
                        &mut x.key_track,
                        0.0,
                        1.0,
                        0.0,
                        false,
                        &pct,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fvel",
                        "Velocity",
                        &mut x.vel_filter,
                        0.0,
                        4.0,
                        0.0,
                        false,
                        &oct,
                    )
                });
            });
        });
        ui.add_space(DRAW_GAP);
        filter_curve(ui, th, inst, vec2(CURVE_W, 2.0 * kh + GAP));
    });
}

/// The output stage: level, how velocity sets loudness, accents.
fn amp(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit, iw: f32, kh: f32) {
    caption_line(ui, th, "output");
    let cw = iw / 2.0;
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "gain",
                "Level",
                &mut inst.gain_db,
                -40.0,
                6.0,
                -10.0,
                false,
                &|v| format!("{v:.1} dB"),
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "vel",
                "Velocity",
                &mut inst.ext.velocity,
                0.0,
                1.0,
                1.0,
                false,
                &pct,
            )
        });
    });
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "accent",
                "Accent",
                &mut inst.ext.accent,
                0.0,
                1.0,
                0.0,
                false,
                &pct,
            )
        });
        cell_mid(ui, cw, kh, 28.0, |ui| {
            ui.label(
                RichText::new("loud notes\n(vel 100+)")
                    .size(10.0)
                    .color(c(th.text_dim)),
            );
        });
    });
}

fn envelopes(
    ui: &mut egui::Ui,
    th: &Theme,
    inst: &mut Instrument,
    ed: &mut Edit,
    iw: f32,
    kh: f32,
) {
    caption_line(ui, th, "amp ——   filter - - -");
    let draw_w = iw - LABEL_W - 4.0 * CELL - DRAW_GAP;
    let cw = CELL;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.vertical(|ui| {
            row(ui, |ui| {
                row_label(ui, th, "AMP", LABEL_W, kh);
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "aa",
                        "Attack",
                        &mut inst.attack,
                        0.001,
                        4.0,
                        0.005,
                        true,
                        &secs,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "ad",
                        "Decay",
                        &mut inst.decay,
                        0.01,
                        6.0,
                        0.25,
                        true,
                        &secs,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "as",
                        "Sustain",
                        &mut inst.sustain,
                        0.0,
                        1.0,
                        0.7,
                        false,
                        &pct,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "ar",
                        "Release",
                        &mut inst.release,
                        0.01,
                        8.0,
                        0.25,
                        true,
                        &secs,
                    )
                });
            });
            row(ui, |ui| {
                row_label(ui, th, "FILTER", LABEL_W, kh);
                let x = &mut inst.ext;
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fa",
                        "Attack",
                        &mut x.f_attack,
                        0.001,
                        4.0,
                        0.005,
                        true,
                        &secs,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fd",
                        "Decay",
                        &mut x.f_decay,
                        0.01,
                        6.0,
                        0.4,
                        true,
                        &secs,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fs",
                        "Sustain",
                        &mut x.f_sustain,
                        0.0,
                        1.0,
                        0.0,
                        false,
                        &pct,
                    )
                });
                cell(ui, cw, |ui| {
                    knob(
                        ui,
                        ed,
                        "fr",
                        "Release",
                        &mut x.f_release,
                        0.01,
                        8.0,
                        0.3,
                        true,
                        &secs,
                    )
                });
            });
        });
        ui.add_space(DRAW_GAP);
        env_drawing(ui, th, inst, vec2(draw_w, 2.0 * kh + GAP));
    });
}

fn lfo(ui: &mut egui::Ui, inst: &mut Instrument, ed: &mut Edit, iw: f32, _kh: f32) {
    let x = &mut inst.ext;
    steps(
        ui,
        ed,
        &mut x.lfo_shape,
        &[
            (LfoShape::Sine, "Sine"),
            (LfoShape::Triangle, "Tri"),
            (LfoShape::Square, "Square"),
            (LfoShape::Saw, "Saw"),
            (LfoShape::SampleHold, "S&H"),
        ],
        &[
            "",
            "",
            "",
            "",
            "Sample and hold: a new random level each cycle",
        ],
    );
    let cw = iw / 6.0;
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "lrate",
                "Rate",
                &mut x.lfo_rate,
                0.05,
                30.0,
                5.0,
                true,
                &|v| format!("{v:.2} Hz"),
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "ldepth",
                "Depth",
                &mut x.lfo_depth,
                0.0,
                1.0,
                0.0,
                false,
                &pct,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "lwheel",
                "Mod Wheel",
                &mut x.wheel_depth,
                0.0,
                1.0,
                1.0,
                false,
                &pct,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "lpitch",
                "Pitch",
                &mut x.lfo_pitch,
                0.0,
                12.0,
                0.5,
                false,
                &st_,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "lfilt",
                "Filter",
                &mut x.lfo_filter,
                0.0,
                4.0,
                0.0,
                false,
                &oct,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "lpwm",
                "PWM",
                &mut x.lfo_pwm,
                0.0,
                0.45,
                0.0,
                false,
                &pct,
            )
        });
    });
}

fn voice(ui: &mut egui::Ui, inst: &mut Instrument, ed: &mut Edit, iw: f32, _kh: f32) {
    let x = &mut inst.ext;
    steps(
        ui,
        ed,
        &mut x.mode,
        &[
            (VoiceMode::Poly, "Poly"),
            (VoiceMode::Mono, "Mono"),
            (VoiceMode::Legato, "Legato"),
        ],
        &[
            "Every note its own voice",
            "One voice; each note starts the envelopes again",
            "One voice; notes played over a held one glide to it without starting again (slide)",
        ],
    );
    let cw = iw / 6.0;
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            let mut v = x.voices as i32;
            knob_i(
                ui,
                ed,
                "voices",
                "Voices",
                &mut v,
                1,
                MAX_VOICES as i32,
                MAX_VOICES as i32,
                "#",
            );
            x.voices = v as u8;
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "glide",
                "Glide",
                &mut x.glide,
                0.0,
                2.0,
                0.0,
                false,
                &secs,
            )
        });
        cell(ui, cw, |ui| {
            let mut u = x.unison as i32;
            knob_i(
                ui,
                ed,
                "unison",
                "Unison",
                &mut u,
                1,
                MAX_UNISON as i32,
                1,
                "#",
            );
            x.unison = u as u8;
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "spread",
                "Spread",
                &mut x.spread,
                0.0,
                60.0,
                15.0,
                false,
                &cents,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "width",
                "Width",
                &mut x.width,
                0.0,
                1.0,
                0.5,
                false,
                &pct,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "bend",
                "Bend",
                &mut x.bend_range,
                0.0,
                24.0,
                2.0,
                false,
                &|v| format!("±{v:.0} st"),
            )
        });
    });
}

fn effects(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit, iw: f32, _kh: f32) {
    caption_line(ui, th, "chorus");
    let cw = iw / 3.0;
    let x = &mut inst.ext;
    row(ui, |ui| {
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "chorus",
                "Mix",
                &mut x.chorus,
                0.0,
                1.0,
                0.0,
                false,
                &pct,
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "crate",
                "Rate",
                &mut x.chorus_rate,
                0.05,
                5.0,
                0.5,
                true,
                &|v| format!("{v:.2} Hz"),
            )
        });
        cell(ui, cw, |ui| {
            knob(
                ui,
                ed,
                "cdepth",
                "Depth",
                &mut x.chorus_depth,
                0.0,
                1.0,
                0.5,
                false,
                &pct,
            )
        });
    });
}

// ---------------------------------------------------------------------
// The waveform preview.

/// The preview's note (A3) and sample rate.
const PREVIEW_PITCH: u8 = 57;
const PREVIEW_RATE: u32 = 48_000;

/// One preview: the voice's sound held at its sustain, and the same with
/// the filter wide open (the ghost), both starting on the same sample.
#[derive(Clone, Default)]
struct Shot {
    out: Vec<f32>,
    open: Vec<f32>,
}

/// The preview worker's state: what is wanted, what is done.
#[derive(Default)]
struct Preview {
    want: u64,
    want_inst: Option<Instrument>,
    done: u64,
    busy: bool,
    shot: Shot,
}

type Shared = std::sync::Arc<std::sync::Mutex<Preview>>;

fn inst_key(inst: &Instrument) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{inst:?}").hash(&mut h);
    h.finish()
}

/// The latest preview for `inst`; a new one is worked out off the UI
/// thread when the settings have changed (the old one shows meanwhile).
fn preview_for(ctx: &egui::Context, inst: &Instrument) -> (Shot, bool) {
    let id = egui::Id::new("synth-preview");
    let shared: Shared = ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(id).clone());
    let key = inst_key(inst);
    let mut p = shared.lock().unwrap_or_else(|e| e.into_inner());
    if p.want != key {
        p.want = key;
        p.want_inst = Some(*inst);
        if !p.busy {
            p.busy = true;
            let shared = shared.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let (k, inst) = {
                    let mut p = shared.lock().unwrap_or_else(|e| e.into_inner());
                    match p.want_inst {
                        Some(i) if p.done != p.want => (p.want, i),
                        _ => {
                            p.busy = false;
                            return;
                        }
                    }
                };
                let shot = render_preview(&inst);
                {
                    let mut p = shared.lock().unwrap_or_else(|e| e.into_inner());
                    p.done = k;
                    p.shot = shot;
                }
                ctx.request_repaint();
            });
        }
    }
    (p.shot.clone(), p.done == key)
}

/// The voice's sound for a held A3 once its envelopes have settled:
/// rendered with the synth's own engine (oscillators, mix, filter and
/// its envelope, the amp at sustain), the LFO and chorus left out so the
/// shape holds still. Two cycles of its lowest part, from a rising zero.
fn render_preview(inst: &Instrument) -> Shot {
    use af2_engine::synth::{note, Synth};
    let mut still = *inst;
    still.ext.lfo_depth = 0.0;
    still.ext.chorus = 0.0;
    still.ext.glide = 0.0;
    let mut open = still;
    open.cutoff = 18_000.0;
    open.resonance = 0.0;
    open.ext.filter_type = FilterType::LowPass;
    open.ext.filter_kind = FilterKind::Classic;
    open.ext.drive = 0.0;
    open.ext.filter_env = 0.0;
    open.ext.key_track = 0.0;
    open.ext.vel_filter = 0.0;
    open.ext.lfo_filter = 0.0;
    open.ext.accent = 0.0;

    let n = note(0.0, 1.0e4, PREVIEW_PITCH, 100, 0.0);
    let x = inst.ext;
    let semis = |o: i8, s: i8, cents: f32| {
        2f32.powf((f32::from(o) * 12.0 + f32::from(s) + cents / 100.0) / 12.0)
    };
    let r1 = semis(x.osc1_octave, x.osc1_semi, x.osc1_fine);
    let r2 = semis(x.osc2_octave, x.osc2_semi, 0.0);
    let mut lo = r1;
    if x.osc2_level > 0.0 && !x.sync {
        lo = lo.min(r2);
    }
    if x.sub > 0.0 {
        lo = lo.min(r1 * 0.5);
    }
    let rate = PREVIEW_RATE as f32;
    let period = (rate / (n.freq * lo).max(10.0)).clamp(8.0, 6000.0);
    let span = (2.0 * period).round() as usize;
    // Past the attacks and decays, so both envelopes sit at sustain.
    let settle = f64::from(
        (inst.attack + inst.decay)
            .max(x.f_attack + x.f_decay)
            .clamp(0.0, 20.0),
    ) + 0.05;
    let warm = (0.03 * rate) as usize;
    let len = warm + period.ceil() as usize + span + 4;
    let run = |i: Instrument| -> Vec<f32> {
        let mut s = Synth::new(i, vec![n], PREVIEW_RATE);
        let mut l = vec![0.0f32; len];
        let mut r = vec![0.0f32; len];
        s.render(settle, 1.0 / f64::from(PREVIEW_RATE), &mut l, &mut r);
        l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()
    };
    let out = run(still);
    let dry = run(open);
    // Start on the steepest rising crossing of the middle within a cycle.
    let p = period.ceil() as usize;
    let mid = out[warm..warm + p].iter().sum::<f32>() / p as f32;
    let mut start = warm;
    let mut best = f32::MIN;
    for i in warm.max(1)..warm + p {
        if out[i - 1] < mid && out[i] >= mid {
            let slope = out[i] - out[i - 1];
            if slope > best {
                best = slope;
                start = i;
            }
        }
    }
    let end = (start + span).min(len);
    Shot {
        out: out[start..end].iter().map(|v| v - mid).collect(),
        open: dry[start..end].to_vec(),
    }
}

/// The waveform screen: the output, and the filter's effect as a ghost
/// of the sound before it.
fn scope(ui: &mut egui::Ui, th: &Theme, inst: &Instrument, size: egui::Vec2) {
    let (shot, fresh) = preview_for(ui.ctx(), inst);
    let (rect, p) = screen(ui, th, size);
    let mid = rect.center().y;
    p.line_segment(
        [pos2(rect.left(), mid), pos2(rect.right(), mid)],
        Stroke::new(1.0_f32, ca(th.text_dim, 60)),
    );
    let peak = shot
        .out
        .iter()
        .chain(&shot.open)
        .fold(0.0f32, |m, v| m.max(v.abs()))
        .max(1e-4);
    let inner = rect.shrink2(vec2(6.0, 18.0));
    let trace = |v: &[f32]| -> Vec<Pos2> {
        let n = v.len().max(2);
        // At most about two points per pixel.
        let step = (n as f32 / (inner.width() * 2.0)).max(1.0);
        let mut pts = Vec::new();
        let mut i = 0.0f32;
        while (i as usize) < v.len() {
            let k = i as usize;
            pts.push(pos2(
                inner.left() + inner.width() * k as f32 / (n - 1) as f32,
                mid - v[k] / peak * inner.height() * 0.5,
            ));
            i += step;
        }
        pts
    };
    let any = shot.out.iter().any(|v| v.abs() > 1e-5);
    if !shot.open.is_empty() {
        p.add(Shape::line(
            trace(&shot.open),
            Stroke::new(1.0_f32, ca(th.text_dim, 110)),
        ));
    }
    if any {
        let pts = trace(&shot.out);
        if th.dark && th.glow > 0.0 {
            p.add(Shape::line(
                pts.clone(),
                Stroke::new(5.0_f32, ca(th.accent, (34.0 * th.glow) as u8)),
            ));
        }
        p.add(Shape::line(pts, Stroke::new(1.8_f32, c(th.accent))));
    } else {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            if shot.out.is_empty() { "…" } else { "silent at sustain" },
            FontId::proportional(11.0),
            c(th.text_dim),
        );
    }
    p.text(
        rect.left_top() + vec2(6.0, 4.0),
        Align2::LEFT_TOP,
        "A3 held",
        FontId::proportional(10.0),
        c(th.text_dim),
    );
    p.text(
        rect.right_top() + vec2(-6.0, 4.0),
        Align2::RIGHT_TOP,
        "out",
        FontId::proportional(10.0),
        c(th.accent),
    );
    p.text(
        rect.right_bottom() + vec2(-6.0, -4.0),
        Align2::RIGHT_BOTTOM,
        "before filter",
        FontId::proportional(10.0),
        ca(th.text_dim, 200),
    );
    if !fresh {
        p.circle_filled(
            rect.left_bottom() + vec2(9.0, -9.0),
            2.5,
            ca(th.accent, 160),
        );
    }
}

/// The drawing's frame: a dark screen with a faint grid.
fn screen(ui: &mut egui::Ui, th: &Theme, size: egui::Vec2) -> (Rect, egui::Painter) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, c(th.wave_background));
    p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, ca(th.border, 200)));
    for k in 1..4 {
        let y = rect.top() + rect.height() * k as f32 / 4.0;
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0_f32, ca(th.text_dim, 28)),
        );
    }
    (rect, p)
}

/// |H| of the filter at `f` Hz (analog prototypes of the two filters).
fn response(inst: &Instrument, f: f32) -> f32 {
    let w = f / inst.cutoff.max(1.0);
    let res = inst.resonance.clamp(0.0, 1.0);
    // Complex numbers as (re, im).
    let mul = |a: (f32, f32), b: (f32, f32)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    let div = |a: (f32, f32), b: (f32, f32)| {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    };
    let mag = |a: (f32, f32)| (a.0 * a.0 + a.1 * a.1).sqrt();
    let s = (0.0, w);
    match inst.ext.filter_kind {
        FilterKind::Classic => {
            let k = 2.0 - 1.9 * res;
            let den = (1.0 - w * w, k * w);
            let num = match inst.ext.filter_type {
                FilterType::LowPass => (1.0, 0.0),
                FilterType::BandPass => s,
                FilterType::HighPass => mul(s, s),
            };
            mag(div(num, den))
        }
        FilterKind::Ladder => {
            let k = 3.96 * res;
            let g = div((1.0, 0.0), (1.0, w)); // one stage
            let g2 = mul(g, g);
            let g4 = mul(g2, g2);
            let den = (1.0 + k * g4.0, k * g4.1);
            let num = match inst.ext.filter_type {
                FilterType::LowPass => g4,
                // 4 (G² - 2G³ + G⁴) and the 4-pole high-pass.
                FilterType::BandPass => {
                    let g3 = mul(g2, g);
                    (
                        4.0 * (g2.0 - 2.0 * g3.0 + g4.0),
                        4.0 * (g2.1 - 2.0 * g3.1 + g4.1),
                    )
                }
                FilterType::HighPass => {
                    let one_m = (1.0 - g.0, -g.1);
                    let a = mul(one_m, one_m);
                    mul(a, a)
                }
            };
            mag(div(num, den)) * (1.0 + 0.6 * k)
        }
    }
}

fn filter_curve(ui: &mut egui::Ui, th: &Theme, inst: &Instrument, size: egui::Vec2) {
    ui.vertical(|ui| {
        let (rect, p) = screen(ui, th, size);
        let (lo, hi) = (20f32.ln(), 20_000f32.ln());
        let db_top = 24.0f32;
        let db_bot = -48.0f32;
        let y_of = |db: f32| {
            let t = ((db - db_top) / (db_bot - db_top)).clamp(0.0, 1.0);
            rect.top() + 4.0 + t * (rect.height() - 8.0)
        };
        // 0 dB line and the cutoff.
        p.line_segment(
            [pos2(rect.left(), y_of(0.0)), pos2(rect.right(), y_of(0.0))],
            Stroke::new(1.0_f32, ca(th.text_dim, 70)),
        );
        let fx = rect.left() + rect.width() * (inst.cutoff.max(20.0).ln() - lo) / (hi - lo);
        p.line_segment(
            [pos2(fx, rect.top()), pos2(fx, rect.bottom())],
            Stroke::new(1.0_f32, ca(th.accent, 60)),
        );
        let n = 96;
        let pts: Vec<Pos2> = (0..=n)
            .map(|k| {
                let u = k as f32 / n as f32;
                let f = (lo + (hi - lo) * u).exp();
                let db = 20.0 * response(inst, f).max(1e-6).log10();
                pos2(rect.left() + rect.width() * u, y_of(db))
            })
            .collect();
        // A soft wash under the curve (strips, as the shape is not convex).
        for w in pts.windows(2) {
            p.add(Shape::convex_polygon(
                vec![
                    w[0],
                    w[1],
                    pos2(w[1].x, rect.bottom()),
                    pos2(w[0].x, rect.bottom()),
                ],
                ca(th.accent, 26),
                Stroke::NONE,
            ));
        }
        p.add(Shape::line(pts, Stroke::new(1.6_f32, c(th.accent))));
        p.text(
            rect.left_top() + vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            hz(inst.cutoff as f64),
            FontId::proportional(10.0),
            c(th.text_dim),
        );
    });
}

fn env_drawing(ui: &mut egui::Ui, th: &Theme, inst: &Instrument, size: egui::Vec2) {
    let (rect, p) = screen(ui, th, size);
    let x = inst.ext;
    let amp = [inst.attack, inst.decay, inst.sustain, inst.release];
    let filt = [x.f_attack, x.f_decay, x.f_sustain, x.f_release];
    // Both on one time scale: the longer attack + decay + hold + release.
    let hold = 0.25f32;
    let span = |e: &[f32; 4]| e[0] + e[1] + e[3];
    let total = (span(&amp).max(span(&filt)) * (1.0 + hold)).max(0.05);
    let held = (amp[0] + amp[1]).max(filt[0] + filt[1]) + total * hold / (1.0 + hold);
    let to = |t: f32, v: f32| {
        pos2(
            rect.left() + 4.0 + (rect.width() - 8.0) * (t / total).min(1.0),
            rect.bottom() - 6.0 - (rect.height() - 14.0) * v.clamp(0.0, 1.0),
        )
    };
    let path = |e: &[f32; 4]| -> Vec<Pos2> {
        let (a, d, s, r) = (e[0].max(0.0005), e[1].max(0.001), e[2], e[3].max(0.001));
        let lvl = |t: f32| {
            if t < a {
                t / a
            } else if t < a + d {
                1.0 - (1.0 - s) * (t - a) / d
            } else {
                s
            }
        };
        let from = lvl(held);
        vec![
            to(0.0, 0.0),
            to(a.min(held), lvl(a.min(held))),
            to((a + d).min(held), lvl((a + d).min(held))),
            to(held, from),
            to(held + r, 0.0),
        ]
    };
    p.line_segment(
        [
            pos2(to(held, 0.0).x, rect.top()),
            pos2(to(held, 0.0).x, rect.bottom()),
        ],
        Stroke::new(1.0_f32, ca(th.text_dim, 50)),
    );
    if x.filter_env != 0.0 || x.osc2_env != 0.0 {
        p.add(Shape::dashed_line(
            &path(&filt),
            Stroke::new(1.2_f32, c(th.accent_alt)),
            4.0,
            3.0,
        ));
    }
    p.add(Shape::line(path(&amp), Stroke::new(1.6_f32, c(th.accent))));
    p.text(
        rect.right_top() + vec2(-6.0, 4.0),
        Align2::RIGHT_TOP,
        "amp / filter",
        FontId::proportional(10.0),
        c(th.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_draws_every_model_held_and_quickly() {
        for m in SynthModel::ALL {
            let t = std::time::Instant::now();
            let s = render_preview(&m.preset());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            assert!(!s.out.is_empty() && s.out.len() == s.open.len(), "{m:?}");
            let peak = s.out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            assert!(peak > 1e-3 && peak.is_finite(), "{m:?} peak {peak}");
            // Cheap enough to redo on every change (well under a frame
            // in release; debug builds are slower).
            assert!(ms < 500.0, "{m:?} took {ms} ms");
        }
    }

    #[test]
    fn the_preview_shows_two_cycles_of_the_lowest_part() {
        let mut i = SynthModel::Default.preset();
        i.ext.osc2_level = 0.0;
        i.ext.osc1_level = 1.0;
        i.wave = Wave::Sine;
        let s = render_preview(&i);
        // A3 at 48 kHz: about 218 samples a cycle.
        assert!((s.out.len() as f32 - 2.0 * 48_000.0 / 220.0).abs() < 2.0);
        // A sub an octave down doubles it.
        i.ext.sub = 0.5;
        let s2 = render_preview(&i);
        assert!((s2.out.len() as f32 - 4.0 * 48_000.0 / 220.0).abs() < 2.0);
    }
}
