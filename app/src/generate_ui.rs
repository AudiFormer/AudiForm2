// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! Effects > Generate > Noise and Tones: AudiForm 2's own generator
//! windows, drawn from the theme (vertical faders, dials and value boxes;
//! no stock sliders).
//!
//! - Noise: a colour (white, pink, brown, grey, blue, violet) shown as its
//!   power spectral density, a stereo style (spatial with a delay,
//!   independent channels, mono, inverse), Intensity and DC Offset faders.
//! - Tones: a base frequency with frequency modulation, a waveform with a
//!   shape control and a live preview, five frequency components (each a
//!   multiple of the base, with its own level), a Volume fader, an
//!   optional sweep from the Start settings to the End settings, and
//!   advanced DC offset and phase.
//!
//! Both have presets (built-in and saved), a duration, Replace / Insert /
//! Overlap with the selected audio, which channels to write, Preview
//! (plays the result without touching the file) and OK.

use std::path::PathBuf;

use egui::{pos2, vec2, Align2, FontId, Pos2, RichText, Sense, Stroke};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use af27_core::AudioBuffer;
use af27_dsp::generate as gen;

use crate::fx_windows::layout;
use crate::graph::{self, Plot};
use crate::state::{Mode, State, View};
use crate::theme::{c, ca, Theme};
use crate::widgets::{self, VFader};

// ------------------------------------------------------------ settings

/// How generated audio meets the file.
pub const PLACES: [&str; 3] = ["Replace", "Insert", "Overlap"];
const PLACE_TIPS: [&str; 3] = [
    "Replace the selected audio (or insert at the playhead with no selection)",
    "Insert at the playhead (or the start of the selection), pushing the rest later",
    "Mix into the audio from the playhead (or the selection) on",
];

const STYLES: [&str; 4] = ["Spatial stereo", "Independent", "Mono", "Inverse"];
const STYLE_TIPS: [&str; 4] = [
    "One noise reaching each channel a little later than the one before: wide and airy",
    "A noise of its own on every channel",
    "The same noise on every channel",
    "The same noise, upside down on every other channel",
];

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NoiseUi {
    pub color: usize,
    pub style: usize,
    /// Spatial stereo's delay between channels, ms.
    pub delay_ms: f64,
    /// RMS level, dBFS.
    pub intensity: f64,
    pub dc: f64,
    pub secs: f64,
}

impl Default for NoiseUi {
    fn default() -> Self {
        NoiseUi {
            color: 1,
            style: 1,
            delay_ms: 5.0,
            intensity: -20.0,
            dc: 0.0,
            secs: 5.0,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CompUi {
    pub on: bool,
    pub db: f64,
    pub mult: f64,
}

impl Default for CompUi {
    fn default() -> Self {
        CompUi {
            on: false,
            db: -12.0,
            mult: 1.0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToneSetUi {
    pub base: f64,
    pub mod_depth: f64,
    pub mod_rate: f64,
    pub comps: [CompUi; gen::COMPONENTS],
    pub volume: f64,
}

impl Default for ToneSetUi {
    fn default() -> Self {
        let mut comps = [CompUi::default(); gen::COMPONENTS];
        for (k, c) in comps.iter_mut().enumerate() {
            c.mult = (k + 1) as f64;
        }
        comps[0].on = true;
        comps[0].db = 0.0;
        ToneSetUi {
            base: 440.0,
            mod_depth: 0.0,
            mod_rate: 0.0,
            comps,
            volume: -6.0,
        }
    }
}

impl ToneSetUi {
    fn to_set(&self) -> gen::ToneSet {
        gen::ToneSet {
            base: self.base,
            mod_depth: self.mod_depth,
            mod_rate: self.mod_rate,
            components: std::array::from_fn(|k| gen::Component {
                on: self.comps[k].on,
                amp_db: self.comps[k].db,
                multiplier: self.comps[k].mult,
            }),
            volume_db: self.volume,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToneUi {
    pub start: ToneSetUi,
    pub end: ToneSetUi,
    pub sweep: bool,
    pub wave: usize,
    /// 0..100, 50 plain.
    pub shape: f64,
    pub dc: f64,
    pub phase: f64,
    pub channel_phase: f64,
    pub secs: f64,
}

impl Default for ToneUi {
    fn default() -> Self {
        ToneUi {
            start: ToneSetUi::default(),
            end: ToneSetUi::default(),
            sweep: false,
            wave: 0,
            shape: 50.0,
            dc: 0.0,
            phase: 0.0,
            channel_phase: 0.0,
            secs: 5.0,
        }
    }
}

impl ToneUi {
    pub fn spec(&self) -> gen::ToneSpec {
        gen::ToneSpec {
            start: self.start.to_set(),
            end: self.sweep.then(|| self.end.to_set()),
            wave: gen::Wave::ALL[self.wave.min(gen::Wave::ALL.len() - 1)],
            shape: self.shape / 100.0,
            dc_percent: self.dc,
            phase_deg: self.phase,
            channel_phase_deg: self.channel_phase,
        }
    }
}

impl NoiseUi {
    pub fn spec(&self) -> gen::NoiseSpec {
        gen::NoiseSpec {
            color: gen::Noise::ALL[self.color.min(gen::Noise::ALL.len() - 1)],
            style: match self.style {
                0 => gen::NoiseStyle::Spatial(self.delay_ms),
                2 => gen::NoiseStyle::Mono,
                3 => gen::NoiseStyle::Inverse,
                _ => gen::NoiseStyle::Independent,
            },
            level_db: self.intensity,
            dc_percent: self.dc,
            ..gen::NoiseSpec::default()
        }
    }
}

/// The generator windows' state (kept while the app runs).
pub struct GenerateUi {
    pub noise_open: bool,
    pub tones_open: bool,
    pub noise: NoiseUi,
    pub tones: ToneUi,
    /// Which end of a sweep the Tones window is editing.
    pub editing_end: bool,
    pub place: usize,
    /// Channels to write (by index); others are left alone.
    pub channels: Vec<bool>,
    pub preset_name: String,
}

impl Default for GenerateUi {
    fn default() -> Self {
        GenerateUi {
            noise_open: false,
            tones_open: false,
            noise: NoiseUi::default(),
            tones: ToneUi::default(),
            editing_end: false,
            place: 0,
            channels: vec![true; 2],
            preset_name: String::new(),
        }
    }
}

// ------------------------------------------------------------ presets

fn noise_presets() -> Vec<(&'static str, NoiseUi)> {
    let d = NoiseUi::default();
    vec![
        ("Default", d.clone()),
        (
            "White, independent",
            NoiseUi {
                color: 0,
                ..d.clone()
            },
        ),
        (
            "Pink, spatial",
            NoiseUi {
                style: 0,
                delay_ms: 8.0,
                ..d.clone()
            },
        ),
        (
            "Brown rumble",
            NoiseUi {
                color: 2,
                style: 2,
                intensity: -18.0,
                ..d.clone()
            },
        ),
        (
            "Grey (even loudness)",
            NoiseUi {
                color: 3,
                intensity: -24.0,
                ..d.clone()
            },
        ),
        (
            "Violet hiss",
            NoiseUi {
                color: 5,
                intensity: -30.0,
                ..d
            },
        ),
    ]
}

fn tone_presets() -> Vec<(&'static str, ToneUi)> {
    let d = ToneUi::default();
    let mut test = d.clone();
    test.start.base = 1000.0;
    test.start.volume = -18.0;
    let mut sweep = d.clone();
    sweep.start.base = 20.0;
    sweep.end = sweep.start.clone();
    sweep.end.base = 20_000.0;
    sweep.sweep = true;
    sweep.secs = 10.0;
    let mut siren = d.clone();
    siren.start.base = 700.0;
    siren.start.mod_depth = 250.0;
    siren.start.mod_rate = 0.5;
    let mut organ = d.clone();
    for (k, db) in [0.0, -6.0, -10.0, -14.0, -18.0].iter().enumerate() {
        organ.start.comps[k] = CompUi {
            on: true,
            db: *db,
            mult: [1.0, 2.0, 3.0, 4.0, 6.0][k],
        };
    }
    organ.start.base = 220.0;
    organ.start.volume = -14.0;
    let mut dial = d.clone();
    dial.start.base = 350.0;
    dial.start.comps[1] = CompUi {
        on: true,
        db: 0.0,
        mult: 440.0 / 350.0,
    };
    dial.start.volume = -12.0;
    let mut square = d.clone();
    square.wave = 2;
    square.start.base = 110.0;
    square.start.volume = -18.0;
    vec![
        ("Default (440 Hz sine)", d),
        ("1 kHz test tone, -18 dB", test),
        ("Sweep 20 Hz to 20 kHz", sweep),
        ("Siren", siren),
        ("Organ (harmonics)", organ),
        ("Dial tone (350 + 440 Hz)", dial),
        ("Square bass", square),
    ]
}

fn presets_dir(kind: &str) -> PathBuf {
    crate::state::data_dir().join("generate-presets").join(kind)
}

fn saved_presets(kind: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(presets_dir(kind))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()? != "json" {
                return None;
            }
            Some(p.file_stem()?.to_string_lossy().into_owned())
        })
        .collect();
    v.sort();
    v
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|ch| if r#"\/:*?"<>|"#.contains(ch) { '_' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

/// The presets row: pick a built-in or saved preset, save the current
/// settings under a name, delete a saved one.
fn presets_row<T: Serialize + DeserializeOwned + Clone + PartialEq>(
    ui: &mut egui::Ui,
    kind: &str,
    builtin: &[(&'static str, T)],
    cur: &mut T,
    name: &mut String,
    th: &Theme,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Presets").color(c(th.text_dim)));
        let shown = builtin
            .iter()
            .find(|(_, v)| v == cur)
            .map_or("(custom)".to_string(), |(n, _)| n.to_string());
        let saved = saved_presets(kind);
        egui::ComboBox::from_id_salt(("gen-presets", kind))
            .selected_text(shown)
            .width(190.0)
            .show_ui(ui, |ui| {
                for (n, v) in builtin {
                    if ui.selectable_label(v == cur, *n).clicked() {
                        *cur = v.clone();
                    }
                }
                if !saved.is_empty() {
                    ui.separator();
                }
                for n in &saved {
                    if ui.selectable_label(false, n).clicked() {
                        let p = presets_dir(kind).join(format!("{n}.json"));
                        if let Some(v) = std::fs::read_to_string(p)
                            .ok()
                            .and_then(|t| serde_json::from_str(&t).ok())
                        {
                            *cur = v;
                            *name = n.clone();
                        }
                    }
                }
            });
        ui.add(
            egui::TextEdit::singleline(name)
                .hint_text("Name")
                .desired_width(110.0),
        );
        let n = safe_name(name);
        if widgets::button(ui, "Save", false)
            .on_hover_text("Save these settings as a preset")
            .clicked()
            && !n.is_empty()
        {
            let dir = presets_dir(kind);
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(t) = serde_json::to_string_pretty(cur) {
                let _ = std::fs::write(dir.join(format!("{n}.json")), t);
            }
        }
        if saved.contains(&n)
            && widgets::button(ui, "Delete", false)
                .on_hover_text("Delete the saved preset of this name")
                .clicked()
        {
            let _ = std::fs::remove_file(presets_dir(kind).join(format!("{n}.json")));
        }
    });
}

// ------------------------------------------------------------ windows

/// The generator windows, drawn every frame (they show only when open).
pub fn windows(ctx: &egui::Context, st: &mut State) {
    if st.generate.noise_open {
        noise_window(ctx, st);
    }
    if st.generate.tones_open {
        tones_window(ctx, st);
    }
}

fn doc_shape(st: &State) -> (u32, usize) {
    st.doc()
        .map_or((48_000, 2), |d| (d.buf.rate, d.buf.num_channels()))
}

/// A titled group `w` wide (the effect windows' frame), at least
/// `min_h` tall, its contents centred.
fn group(
    ui: &mut egui::Ui,
    th: &Theme,
    title: &str,
    w: f32,
    min_h: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let mut add = Some(add);
    layout::groups_min(ui, th, w, &[(title, 1.0)], min_h, |ui, _| {
        if let Some(f) = add.take() {
            f(ui);
        }
    });
}

/// Duration, placement, channels, Preview and OK / Close.
fn footer(
    ui: &mut egui::Ui,
    st: &mut State,
    th: &Theme,
    secs: f64,
    make: &dyn Fn(u32, usize, f64) -> AudioBuffer,
) -> (Option<f64>, bool) {
    let (rate, ch) = doc_shape(st);
    let mut new_secs = None;
    let mut close = false;
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Duration").color(c(th.text_dim)));
        let mut s = secs;
        if widgets::value_box(
            ui,
            ui.id().with("dur"),
            &mut s,
            0.01,
            3600.0,
            true,
            80.0,
            &|v| crate::widgets::fmt_unit(v, "s"),
        )
        .changed()
        {
            new_secs = Some(s);
        }
        if st.doc().and_then(|d| d.sel).is_some()
            && widgets::button(ui, "Selection", false)
                .on_hover_text("Make it as long as the selection")
                .clicked()
        {
            if let Some((a, b)) = st.doc().and_then(|d| d.sel) {
                new_secs = Some((b - a).abs().max(0.01));
            }
        }
        ui.add_space(12.0);
        widgets::segmented(ui, &mut st.generate.place, &PLACES, &PLACE_TIPS);
        ui.add_space(12.0);
        ui.label(RichText::new("Channels").color(c(th.text_dim)));
        st.generate.channels.resize(ch.max(1), true);
        let names = channel_names(ch);
        for (k, n) in names.iter().enumerate() {
            widgets::switch(ui, &mut st.generate.channels[k], n);
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let playing = st.is_playing();
        if widgets::button(ui, if playing { "■ Stop" } else { "▶ Preview" }, playing)
            .on_hover_text("Play what would be generated (the file is not changed)")
            .clicked()
        {
            if playing {
                st.stop();
            } else {
                preview(st, make(rate, ch, secs.min(15.0)));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::button(ui, "Close", false).clicked() {
                close = true;
            }
            if widgets::button(ui, "OK", true)
                .on_hover_text("Generate into the file")
                .clicked()
            {
                let buf = make(rate, ch, secs);
                let place = st.generate.place;
                let chans = st.generate.channels.clone();
                place_generated(st, "Generate", buf, place, &chans);
                close = true;
            }
        });
    });
    (new_secs, close)
}

fn channel_names(n: usize) -> Vec<String> {
    match n {
        1 => vec!["Mono".into()],
        2 => vec!["L".into(), "R".into()],
        _ => af27_dsp::wide::labels(n),
    }
}

/// Plays a generated buffer on its own (the next Play goes back to the
/// file).
fn preview(st: &mut State, buf: AudioBuffer) {
    let len = buf.duration_secs();
    if let Some(eng) = &mut st.engine {
        eng.set_program(af27_engine::buffer_program(std::sync::Arc::new(buf)));
        eng.play(0.0, None, false, len);
    }
    st.session_version += 1;
}

/// Puts generated audio into the current file (a new one if none is
/// open): replacing the selection, inserted, or mixed over what is there,
/// on the chosen channels only. One undo step.
pub fn place_generated(
    st: &mut State,
    label: &str,
    buf: AudioBuffer,
    place: usize,
    chans: &[bool],
) {
    st.mode = Mode::Waveform;
    if st.doc().is_none() {
        let n = st.docs.len() + 1;
        let id = st.add_doc(
            format!("Untitled {n}"),
            None,
            AudioBuffer::new(buf.rate, buf.num_channels(), 0),
        );
        st.current = Some(id);
    }
    let Some(d) = st.doc_mut() else { return };
    let mut g = af27_dsp::resample::remix(
        &af27_dsp::resample::resample(&buf, d.buf.rate),
        d.buf.num_channels(),
    );
    let rate = d.buf.rate as f64;
    let mut b = (*d.buf).clone();
    let sel = d.sel.map(|_| d.target());
    let at = sel
        .map_or((d.cursor * rate) as usize, |r| r.start)
        .min(b.frames());
    let len = g.frames();
    let keep = |k: usize| chans.get(k).copied().unwrap_or(true);
    match place {
        // Overlap: mixed in, the file growing if it runs past the end.
        2 => {
            if at + len > b.frames() {
                let extra = AudioBuffer::new(b.rate, b.num_channels(), at + len - b.frames());
                let end = b.frames();
                b.insert(end, &extra);
            }
            for (k, ch) in b.channels.iter_mut().enumerate() {
                if keep(k) {
                    for (o, v) in ch[at..at + len].iter_mut().zip(&g.channels[k]) {
                        *o = (*o + v).clamp(-1.0, 1.0);
                    }
                }
            }
        }
        _ => {
            let old = if place == 0 {
                sel.map(|r| b.cut(r))
            } else {
                None
            };
            // Channels left alone keep what was there (the replaced audio,
            // or silence when inserting).
            for (k, ch) in g.channels.iter_mut().enumerate() {
                if !keep(k) {
                    for (i, v) in ch.iter_mut().enumerate() {
                        *v = old
                            .as_ref()
                            .and_then(|o| o.channels.get(k)?.get(i).copied())
                            .unwrap_or(0.0);
                    }
                }
            }
            b.insert(at, &g);
        }
    }
    d.set_buffer(label, b);
    d.sel = Some((at as f64 / rate, (at + len) as f64 / rate));
    d.view = View::fit(d.duration());
}

// ------------------------------------------------------------ noise

fn noise_window(ctx: &egui::Context, st: &mut State) {
    let th = st.theme.clone();
    let mut open = true;
    let mut close = false;
    crate::ui::popup(ctx, egui::Id::new("af2-generate-noise"), "Generate Noise")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let mut name = std::mem::take(&mut st.generate.preset_name);
            presets_row(
                ui,
                "noise",
                &noise_presets(),
                &mut st.generate.noise,
                &mut name,
                &th,
            );
            st.generate.preset_name = name;
            ui.add_space(4.0);
            let n = &mut st.generate.noise;
            // The settings in one column as wide as the graph, the
            // levels beside them as tall as that column.
            const LEFT: f32 = 420.0 + 2.0 * layout::PAD;
            const RIGHT: f32 = 168.0;
            let left_h = layout::last_height(ui, "gen-noise-left", None);
            let hs = layout::columns(ui, &[LEFT, RIGHT], layout::GAP_L, |ui, col| {
                if col == 0 {
                    group(ui, &th, "Color", LEFT, 0.0, |ui| {
                        let names: Vec<&str> = gen::Noise::ALL.iter().map(|c| c.name()).collect();
                        widgets::segmented(ui, &mut n.color, &names, &[]);
                    });
                    ui.add_space(layout::GAP_S);
                    group(ui, &th, "Style", LEFT, 0.0, |ui| {
                        widgets::segmented(ui, &mut n.style, &STYLES, &STYLE_TIPS);
                        ui.add_space(4.0);
                        ui.add_enabled_ui(n.style == 0, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Delay").color(c(th.text_dim)));
                                widgets::value_box(
                                    ui,
                                    ui.id().with("delay"),
                                    &mut n.delay_ms,
                                    0.0,
                                    100.0,
                                    false,
                                    70.0,
                                    &|v| format!("{v:.1} ms"),
                                )
                                .on_hover_text("How much later each channel hears the noise");
                            });
                        });
                    });
                    ui.add_space(layout::GAP_S);
                    group(ui, &th, "Power spectral density", LEFT, 0.0, |ui| {
                        psd_graph(ui, &th, gen::Noise::ALL[n.color.min(5)], n.intensity);
                    });
                } else {
                    group(ui, &th, "Levels", RIGHT, left_h, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.add(
                                VFader::new(&mut n.intensity, -60.0..=0.0)
                                    .label("Intensity")
                                    .unit("dB")
                                    .default_value(-20.0)
                                    .height(214.0)
                                    .ticks(&[0.0, -6.0, -12.0, -20.0, -30.0, -40.0, -60.0]),
                            )
                            .on_hover_text("The noise's RMS level (dBFS)");
                            ui.add(
                                VFader::new(&mut n.dc, -100.0..=100.0)
                                    .label("DC Offset")
                                    .unit("%")
                                    .default_value(0.0)
                                    .height(214.0)
                                    .ticks(&[100.0, 50.0, 0.0, -50.0, -100.0]),
                            )
                            .on_hover_text(
                                "A steady offset added to every sample, % of full scale",
                            );
                        });
                    });
                }
            });
            layout::last_height(ui, "gen-noise-left", Some(hs[0]));
            let spec = st.generate.noise.spec();
            let secs = st.generate.noise.secs;
            let make =
                move |rate: u32, ch: usize, secs: f64| gen::noise_with(rate, ch, secs, &spec);
            let (s, cl) = footer(ui, st, &th, secs, &make);
            if let Some(s) = s {
                st.generate.noise.secs = s;
            }
            close = cl;
        });
    st.generate.noise_open = open && !close;
}

/// The colour's power spectral density, dB against log frequency.
fn psd_graph(ui: &mut egui::Ui, th: &Theme, color: gen::Noise, level: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(420.0, 150.0), Sense::hover());
    let area = egui::Rect::from_min_max(rect.min + vec2(2.0, 4.0), rect.max - vec2(34.0, 16.0));
    // Levels are drawn per hertz against the level at 1 kHz, moved by the
    // intensity so the faders move the curve.
    let plot = Plot {
        rect: area,
        f_lo: 20.0,
        f_hi: 20_000.0,
        linear: false,
        y_lo: -100.0,
        y_hi: 20.0,
    };
    let p = ui.painter_at(rect.expand(2.0));
    graph::backdrop(&p, area, th);
    graph::grid(&p, &plot, th, &[0.0, -20.0, -40.0, -60.0, -80.0], "");
    let offset = level - 30.0;
    let pts: Vec<Pos2> = (0..=96)
        .map(|k| {
            let f = 20.0 * 1000f64.powf(k as f64 / 96.0);
            pos2(plot.x(f), plot.y(color.psd_db(f) + offset))
        })
        .collect();
    graph::curve(&p, &plot, &pts, th, true);
    p.text(
        area.left_top() + vec2(6.0, 4.0),
        Align2::LEFT_TOP,
        format!("{} noise · dB", color.name()),
        FontId::proportional(10.0),
        c(th.text_dim),
    );
}

// ------------------------------------------------------------ tones

fn tones_window(ctx: &egui::Context, st: &mut State) {
    let th = st.theme.clone();
    let mut open = true;
    let mut close = false;
    crate::ui::popup(ctx, egui::Id::new("af2-generate-tones"), "Generate Tones")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let mut name = std::mem::take(&mut st.generate.preset_name);
            presets_row(
                ui,
                "tones",
                &tone_presets(),
                &mut st.generate.tones,
                &mut name,
                &th,
            );
            st.generate.preset_name = name;
            ui.add_space(4.0);
            let g = &mut st.generate;
            ui.horizontal(|ui| {
                let mut on = g.tones.sweep;
                if widgets::switch(ui, &mut on, "Sweep Frequencies")
                    .on_hover_text(
                        "Glide from the Start settings to the End settings over the duration",
                    )
                    .changed()
                {
                    g.tones.sweep = on;
                    if on && g.tones.end == ToneSetUi::default() {
                        g.tones.end = g.tones.start.clone();
                    }
                    if !on {
                        g.editing_end = false;
                    }
                }
                ui.add_space(10.0);
                ui.add_enabled_ui(g.tones.sweep, |ui| {
                    let mut tab = usize::from(g.editing_end);
                    widgets::segmented(
                        ui,
                        &mut tab,
                        &["Start", "End"],
                        &[
                            "The settings it starts with",
                            "The settings it reaches at the end",
                        ],
                    );
                    g.editing_end = tab == 1;
                    if g.tones.sweep && widgets::button(ui, "Copy Start to End", false).clicked() {
                        g.tones.end = g.tones.start.clone();
                    }
                });
            });
            ui.add_space(4.0);
            let editing_end = g.editing_end && g.tones.sweep;
            let wave = g.tones.wave;
            let shape = g.tones.shape;
            // Base and Waveform in one column, the components beside them
            // as tall as that column.
            const LEFT: f32 = 4.0 * layout::CELL + 2.0 * layout::PAD;
            const RIGHT: f32 = 5.0 * 76.0 + 84.0 + 2.0 * layout::PAD;
            let left_h = layout::last_height(ui, "gen-tones-left", None);
            let hs = layout::columns(ui, &[LEFT, RIGHT], layout::GAP_L, |ui, col| {
                if col == 0 {
                    let set = if editing_end {
                        &mut g.tones.end
                    } else {
                        &mut g.tones.start
                    };
                    group(ui, &th, "Base", LEFT, 0.0, |ui| {
                        let iw = ui.available_width();
                        let id = ui.id();
                        layout::row(ui, iw, &[1.4, 1.0, 1.0], |ui, k| match k {
                            0 => {
                                widgets::knob(
                                    ui,
                                    id.with("base"),
                                    &mut set.base,
                                    1.0,
                                    20_000.0,
                                    440.0,
                                    true,
                                    86.0,
                                    "Base Frequency",
                                    &[20.0, 200.0, 2000.0, 20_000.0],
                                    &|v| crate::widgets::fmt_unit(v, "Hz"),
                                );
                            }
                            1 => {
                                widgets::knob(
                                    ui,
                                    id.with("depth"),
                                    &mut set.mod_depth,
                                    0.0,
                                    5000.0,
                                    0.0,
                                    false,
                                    66.0,
                                    "Mod Depth",
                                    &[],
                                    &|v| crate::widgets::fmt_unit(v, "Hz"),
                                )
                                .on_hover_text("How far the frequency swings (Hz)");
                            }
                            _ => {
                                widgets::knob(
                                    ui,
                                    id.with("rate"),
                                    &mut set.mod_rate,
                                    0.0,
                                    100.0,
                                    0.0,
                                    false,
                                    66.0,
                                    "Mod Rate",
                                    &[],
                                    &|v| format!("{v:.2} Hz"),
                                )
                                .on_hover_text("How fast it swings (Hz)");
                            }
                        });
                    });
                    ui.add_space(layout::GAP_S);
                    group(ui, &th, "Waveform", LEFT, 0.0, |ui| {
                        let iw = ui.available_width();
                        layout::centred(ui, iw, |ui| {
                            let names: Vec<&str> =
                                gen::Wave::ALL.iter().map(|w| w.name()).collect();
                            widgets::segmented(ui, &mut g.tones.wave, &names, &[]);
                        });
                        ui.add_space(layout::GAP_S);
                        layout::row(ui, iw, &[76.0, iw - 76.0], |ui, k| {
                            if k == 0 {
                                let w = gen::Wave::ALL[g.tones.wave.min(4)];
                                ui.add_enabled_ui(w.has_shape(), |ui| {
                                    widgets::knob(
                                        ui,
                                        ui.id().with("shape"),
                                        &mut g.tones.shape,
                                        1.0,
                                        99.0,
                                        50.0,
                                        false,
                                        66.0,
                                        if w == gen::Wave::Square {
                                            "Duty"
                                        } else {
                                            "Symmetry"
                                        },
                                        &[],
                                        &|v| format!("{v:.0}%"),
                                    )
                                    .on_hover_text(
                                        "The square's duty cycle, or how far the triangle leans",
                                    );
                                });
                            } else {
                                let set = if editing_end {
                                    &g.tones.end
                                } else {
                                    &g.tones.start
                                };
                                wave_preview(ui, &th, set, wave, shape);
                            }
                        });
                    });
                } else {
                    let set = if editing_end {
                        &mut g.tones.end
                    } else {
                        &mut g.tones.start
                    };
                    group(ui, &th, "Frequency components", RIGHT, left_h, |ui| {
                        ui.horizontal_top(|ui| {
                            for k in 0..gen::COMPONENTS {
                                component(ui, &th, set, k);
                                ui.add_space(2.0);
                            }
                            ui.add_space(8.0);
                            ui.add(
                                VFader::new(&mut set.volume, -60.0..=0.0)
                                    .label("Volume")
                                    .unit("dB")
                                    .default_value(-6.0)
                                    .height(150.0)
                                    .ticks(&[0.0, -6.0, -12.0, -24.0, -36.0, -60.0]),
                            );
                        });
                    });
                }
            });
            layout::last_height(ui, "gen-tones-left", Some(hs[0]));
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("Advanced").color(c(th.text_dim)))
                .id_salt("gen-tones-advanced")
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let id = ui.id();
                        widgets::knob(
                            ui,
                            id.with("dc"),
                            &mut g.tones.dc,
                            -100.0,
                            100.0,
                            0.0,
                            false,
                            66.0,
                            "DC Offset",
                            &[],
                            &|v| format!("{v:.1}%"),
                        );
                        widgets::knob(
                            ui,
                            id.with("phase"),
                            &mut g.tones.phase,
                            0.0,
                            360.0,
                            0.0,
                            false,
                            66.0,
                            "Start Phase",
                            &[],
                            &|v| format!("{v:.0}°"),
                        );
                        widgets::knob(
                            ui,
                            id.with("cphase"),
                            &mut g.tones.channel_phase,
                            -180.0,
                            180.0,
                            0.0,
                            false,
                            66.0,
                            "Channel Phase",
                            &[],
                            &|v| format!("{v:.0}°"),
                        )
                        .on_hover_text("Each channel starts this far after the one before");
                    });
                });
            let spec = st.generate.tones.spec();
            let secs = st.generate.tones.secs;
            let make = move |rate: u32, ch: usize, secs: f64| gen::tones(rate, ch, secs, &spec);
            let (s, cl) = footer(ui, st, &th, secs, &make);
            if let Some(s) = s {
                st.generate.tones.secs = s;
            }
            close = cl;
        });
    st.generate.tones_open = open && !close;
}

/// One frequency component's column: Enable, a level fader, its
/// frequency and its multiplier (frequency = base × multiplier).
fn component(ui: &mut egui::Ui, th: &Theme, set: &mut ToneSetUi, k: usize) {
    let base = set.base.max(1e-6);
    let comp = &mut set.comps[k];
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            widgets::switch(ui, &mut comp.on, &format!("{}", k + 1))
                .on_hover_text("Turn this component on or off");
        });
        ui.add_enabled_ui(comp.on, |ui| {
            ui.add(
                VFader::new(&mut comp.db, -120.0..=0.0)
                    .unit("dB")
                    .default_value(0.0)
                    .height(122.0)
                    .ticks(&[0.0, -24.0, -48.0, -72.0, -96.0, -120.0]),
            );
            let id = ui.id().with(("comp", k));
            let mut hz = base * comp.mult;
            if widgets::value_box(
                ui,
                id.with("hz"),
                &mut hz,
                1.0,
                96_000.0,
                true,
                60.0,
                &|v| crate::widgets::fmt_unit(v, "Hz"),
            )
            .on_hover_text("This component's frequency")
            .changed()
            {
                comp.mult = hz / base;
            }
            widgets::value_box(
                ui,
                id.with("mult"),
                &mut comp.mult,
                0.01,
                100.0,
                true,
                60.0,
                &|v| format!("× {v:.3}"),
            )
            .on_hover_text("Its multiple of the base frequency");
        });
        let _ = th;
    });
}

/// Two cycles of the tone (every component on, at its level) as drawn.
fn wave_preview(ui: &mut egui::Ui, th: &Theme, set: &ToneSetUi, wave: usize, shape: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(230.0, 92.0), Sense::hover());
    let p = ui.painter_at(rect);
    graph::backdrop(&p, rect, th);
    let mid = rect.center().y;
    p.line_segment(
        [pos2(rect.left(), mid), pos2(rect.right(), mid)],
        Stroke::new(1.0_f32, ca(th.grid, 120)),
    );
    let w = gen::Wave::ALL[wave.min(4)];
    let amps: Vec<(f64, f64)> = set
        .comps
        .iter()
        .filter(|c| c.on && c.db > -120.0)
        .map(|c| (10f64.powf(c.db / 20.0), c.mult))
        .collect();
    let n = 180;
    let mut vals = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = 2.0 * i as f64 / n as f64;
        let v: f64 = amps
            .iter()
            .map(|(a, m)| a * w.at(t * m, shape / 100.0))
            .sum();
        vals.push(v);
    }
    let peak = vals.iter().fold(1e-9f64, |m, v| m.max(v.abs()));
    let pts: Vec<Pos2> = vals
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                rect.left() + 4.0 + (rect.width() - 8.0) * i as f32 / n as f32,
                mid - (rect.height() / 2.0 - 6.0) * (*v / peak) as f32,
            )
        })
        .collect();
    if th.dark && th.glow > 0.0 {
        p.add(egui::Shape::line(
            pts.clone(),
            Stroke::new(5.0_f32, ca(th.accent, (35.0 * th.glow) as u8)),
        ));
    }
    p.add(egui::Shape::line(pts, Stroke::new(1.8_f32, c(th.accent))));
    p.text(
        rect.left_top() + vec2(5.0, 3.0),
        Align2::LEFT_TOP,
        format!(
            "{} · {}",
            w.name(),
            crate::widgets::fmt_unit(set.base, "Hz")
        ),
        FontId::proportional(9.5),
        c(th.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_settings_round_trip_and_build() {
        for (_, n) in noise_presets() {
            let t = serde_json::to_string(&n).unwrap();
            let back: NoiseUi = serde_json::from_str(&t).unwrap();
            assert!(back == n);
            let b = gen::noise_with(48_000, 2, 0.2, &n.spec());
            assert_eq!(b.num_channels(), 2);
        }
        for (name, t) in tone_presets() {
            let s = serde_json::to_string(&t).unwrap();
            let back: ToneUi = serde_json::from_str(&s).unwrap();
            assert!(back == t, "{name}");
            let b = gen::tones(48_000, 2, 0.2, &t.spec());
            assert!(b.peak() > 0.0, "{name}");
        }
        // Older or partial files fill in the rest.
        let n: NoiseUi = serde_json::from_str(r#"{"color":3}"#).unwrap();
        assert_eq!(n.color, 3);
        assert_eq!(n.secs, NoiseUi::default().secs);
        assert_eq!(safe_name(" a/b:c "), "a_b_c");
    }
}
