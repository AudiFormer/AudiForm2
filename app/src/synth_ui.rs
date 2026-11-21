// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The built-in synth's window, for the track in `st.synth_editor`: a
//! model picker over six sections of dials (Oscillators, Filter,
//! Envelopes, LFO, Voice, Effects), with the filter's curve and the two
//! envelopes drawn. Edits go to the playing synth (and the audition) as
//! they happen; each drag, click or wheel turn is one undo step.

use egui::{pos2, vec2, Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke};

use af27_core::midi::{Instrument, Wave};
use af27_core::synth_model::{
    FilterKind, FilterType, LfoShape, SynthModel, VoiceMode, MAX_UNISON, MAX_VOICES,
};

use crate::state::State;
use crate::theme::{c, ca, lerp, Theme};
use crate::widgets;

/// The dials' size.
const KNOB: f32 = 46.0;
/// The drawings' size.
const DRAW: egui::Vec2 = egui::Vec2::new(210.0, 92.0);

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
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    model_row(ui, th, inst, ed);
    ui.add_space(4.0);
    section(ui, th, "Oscillators", |ui| oscillators(ui, th, inst, ed));
    ui.horizontal_top(|ui| {
        section(ui, th, "Filter", |ui| filter(ui, th, inst, ed));
        section(ui, th, "Envelopes", |ui| envelopes(ui, th, inst, ed));
    });
    ui.horizontal_top(|ui| {
        section(ui, th, "LFO", |ui| lfo(ui, th, inst, ed));
        section(ui, th, "Voice", |ui| voice(ui, th, inst, ed));
        section(ui, th, "Effects", |ui| effects(ui, th, inst, ed));
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

/// A framed section with its title.
fn section(ui: &mut egui::Ui, th: &Theme, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(lerp(th.panel_raised, th.background, 0.35))
        .stroke(Stroke::new(1.0_f32, ca(th.border, 220)))
        .rounding(if th.bevel.is_some() { 0.0 } else { 5.0 })
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(title.to_uppercase())
                        .size(11.0)
                        .strong()
                        .extra_letter_spacing(2.0)
                        .color(c(th.text_dim)),
                );
                add(ui);
            });
        });
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
    format!("{v:+.1} st")
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

fn row_label(ui: &mut egui::Ui, th: &Theme, t: &str) {
    ui.add_sized(
        vec2(34.0, 20.0),
        egui::Label::new(RichText::new(t).size(11.0).strong().color(c(th.accent))),
    );
}

fn oscillators(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    let def = af27_core::synth_model::SynthControls::default();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            row_label(ui, th, "OSC 1");
            steps(ui, ed, &mut inst.wave, &WAVES, &[]);
        });
        let x = &mut inst.ext;
        let mut o = x.osc1_octave as i32;
        knob_i(ui, ed, "o1oct", "Octave", &mut o, -3, 3, 0, "");
        x.osc1_octave = o as i8;
        let mut s = x.osc1_semi as i32;
        knob_i(ui, ed, "o1semi", "Semi", &mut s, -12, 12, 0, "");
        x.osc1_semi = s as i8;
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
        );
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
        );
        ui.separator();
        knob(
            ui, ed, "pw", "Pulse W", &mut x.pw, 0.05, 0.95, def.pw, false, &pct,
        );
        knob(ui, ed, "sub", "Sub", &mut x.sub, 0.0, 1.0, 0.0, false, &pct);
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
        );
    });
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            row_label(ui, th, "OSC 2");
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
        let mut o = inst.ext.osc2_octave as i32;
        knob_i(ui, ed, "o2oct", "Octave", &mut o, -3, 3, 0, "");
        inst.ext.osc2_octave = o as i8;
        let mut s = inst.ext.osc2_semi as i32;
        knob_i(ui, ed, "o2semi", "Semi", &mut s, -12, 12, 0, "");
        inst.ext.osc2_semi = s as i8;
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
        );
        let x = &mut inst.ext;
        knob(
            ui,
            ed,
            "o2lvl",
            "Level",
            &mut x.osc2_level,
            0.0,
            1.0,
            def.osc2_level,
            false,
            &pct,
        );
        ui.separator();
        ui.vertical(|ui| {
            ui.add_space(14.0);
            let r = widgets::switch(ui, &mut x.sync, "Sync")
                .on_hover_text("Oscillator 2 restarts with every cycle of oscillator 1");
            if r.changed() {
                ed.changed = true;
                ed.done = true;
            }
        });
        knob(
            ui,
            ed,
            "sweep",
            "Sweep",
            &mut x.osc2_env,
            -24.0,
            24.0,
            0.0,
            false,
            &st_,
        );
        ui.separator();
        knob(ui, ed, "fm", "FM", &mut x.fm, 0.0, 8.0, 0.0, false, &plain);
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
        );
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
        );
    });
}

fn filter(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    ui.horizontal(|ui| {
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
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
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
                );
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
                );
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
                );
            });
            ui.horizontal(|ui| {
                let x = &mut inst.ext;
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
                );
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
                );
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
                );
            });
        });
        filter_curve(ui, th, inst);
    });
}

fn envelopes(ui: &mut egui::Ui, th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    ui.horizontal(|ui| {
        row_label(ui, th, "AMP");
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
        );
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
        );
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
        );
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
        );
    });
    ui.horizontal(|ui| {
        row_label(ui, th, "FILTER");
        let x = &mut inst.ext;
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
        );
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
        );
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
        );
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
        );
    });
    env_drawing(ui, th, inst);
}

fn lfo(ui: &mut egui::Ui, _th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
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
    ui.horizontal(|ui| {
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
        );
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
        );
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
        );
    });
    ui.horizontal(|ui| {
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
        );
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
        );
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
        );
    });
}

fn voice(ui: &mut egui::Ui, _th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
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
    ui.horizontal(|ui| {
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
        );
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
        );
    });
    ui.horizontal(|ui| {
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
        );
        knob(
            ui,
            ed,
            "vel",
            "Velocity",
            &mut x.velocity,
            0.0,
            1.0,
            1.0,
            false,
            &pct,
        );
        knob(
            ui,
            ed,
            "accent",
            "Accent",
            &mut x.accent,
            0.0,
            1.0,
            0.0,
            false,
            &pct,
        );
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
        );
    });
}

fn effects(ui: &mut egui::Ui, _th: &Theme, inst: &mut Instrument, ed: &mut Edit) {
    ui.horizontal(|ui| {
        let x = &mut inst.ext;
        knob(
            ui,
            ed,
            "chorus",
            "Chorus",
            &mut x.chorus,
            0.0,
            1.0,
            0.0,
            false,
            &pct,
        );
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
        );
    });
    ui.horizontal(|ui| {
        knob(
            ui,
            ed,
            "cdepth",
            "Depth",
            &mut inst.ext.chorus_depth,
            0.0,
            1.0,
            0.5,
            false,
            &pct,
        );
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
        );
    });
}

/// The drawing's frame: a dark screen with a faint grid.
fn screen(ui: &mut egui::Ui, th: &Theme) -> (Rect, egui::Painter) {
    let (rect, _) = ui.allocate_exact_size(DRAW, Sense::hover());
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

fn filter_curve(ui: &mut egui::Ui, th: &Theme, inst: &Instrument) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        let (rect, p) = screen(ui, th);
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

fn env_drawing(ui: &mut egui::Ui, th: &Theme, inst: &Instrument) {
    let (rect, p) = screen(ui, th);
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
