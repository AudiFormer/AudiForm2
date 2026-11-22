// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! File > New Audio File and File > New Multitrack Session: the small
//! dialogs that ask for a name and a format first. Each remembers the
//! choices it was last OK'd with.

use std::path::PathBuf;

use egui::{vec2, Align2, RichText};
use serde::{Deserialize, Serialize};

use af2_core::space::SpaceLayout;

use crate::state::State;
use crate::theme::c;

/// What New Audio File starts with: 48 kHz, stereo, 24-bit, unless the
/// user ticked "Make these my defaults" (`chosen`). Settings saved before
/// that box existed only remembered the last file made, and are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewFileDefaults {
    /// 0 (older settings): 48 kHz.
    pub rate: u32,
    pub channels: usize,
    pub bits: u16,
    /// Set by the user on purpose (else the built-in defaults apply).
    pub chosen: bool,
}

impl NewFileDefaults {
    /// What a new file starts as now.
    pub fn starting(&self) -> NewFileDefaults {
        if self.chosen {
            self.clone()
        } else {
            NewFileDefaults::default()
        }
    }
}

impl Default for NewFileDefaults {
    fn default() -> Self {
        // 48 kHz, stereo, 24-bit: what new files start as (the lead's pick).
        NewFileDefaults {
            rate: 48_000,
            channels: 2,
            bits: 24,
            chosen: false,
        }
    }
}

/// What New Multitrack Session starts with (the last choices).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewSessionDefaults {
    /// Where session folders go (None: Documents\AudiForm 2\Sessions).
    pub folder: Option<PathBuf>,
    /// 0: the audio device's rate.
    pub rate: u32,
    pub bits: u16,
    /// "2.0" for stereo, or a 3D Space layout's label.
    pub mix: String,
}

impl Default for NewSessionDefaults {
    fn default() -> Self {
        NewSessionDefaults {
            folder: None,
            rate: 0,
            bits: 24,
            mix: "2.0".into(),
        }
    }
}

pub fn default_sessions_folder() -> PathBuf {
    let docs = std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Documents"))
        .unwrap_or_else(|| PathBuf::from("."));
    docs.join("AudiForm 2").join("Sessions")
}

#[derive(Debug, Clone)]
pub struct NewFileDraft {
    pub name: String,
    pub rate: u32,
    pub channels: usize,
    pub bits: u16,
    focused: bool,
    /// Opened by Record with no file open: OK starts recording into it.
    pub record_after: bool,
    /// "Make these my defaults" is ticked.
    pub make_default: bool,
}

#[derive(Debug, Clone)]
pub struct NewSessionDraft {
    pub name: String,
    pub folder: PathBuf,
    pub template: Option<PathBuf>,
    pub rate: u32,
    pub bits: u16,
    pub mix: String,
    focused: bool,
}

const RATES: [u32; 10] = [
    8_000, 16_000, 22_050, 32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 192_000,
];

pub fn open_new_file(st: &mut State) {
    let d = st.prefs.new_file.starting();
    let chosen = st.prefs.new_file.chosen;
    let n = st.docs.len() + 1;
    st.new_file_ui = Some(NewFileDraft {
        name: format!("Untitled {n}"),
        // 0 (the old "the device's rate" default) starts at 48 kHz now.
        rate: if d.rate == 0 { 48_000 } else { d.rate },
        channels: d.channels.clamp(1, 8),
        bits: d.bits,
        focused: false,
        record_after: false,
        make_default: chosen,
    });
}

/// Record with no file open in the Waveform editor: the New Audio File
/// dialog first (rate, channels, bit depth), and recording into the new
/// file once it is made. A many-input device (four on an SSL 2+) then
/// records the channels asked for, not all of them.
pub fn open_new_file_to_record(st: &mut State) {
    open_new_file(st);
    if let Some(d) = st.new_file_ui.as_mut() {
        d.record_after = true;
    }
}

pub fn open_new_session(st: &mut State, template: Option<PathBuf>) {
    let d = st.prefs.new_session.clone();
    let folder = d.folder.clone().unwrap_or_else(default_sessions_folder);
    let mut taken: Vec<String> = st
        .sessions
        .parked
        .iter()
        .map(|p| p.session.name.clone())
        .collect();
    taken.push(st.session.name.clone());
    // Also skip names that already have a folder there.
    let mut name = crate::sessions::untitled_name(&taken);
    let mut k = 2;
    while folder.join(&name).exists() {
        name = format!("Untitled Session {k}");
        k += 1;
    }
    st.new_session_ui = Some(NewSessionDraft {
        name,
        folder,
        template,
        // 0 (the old "the device's rate" default) starts at 48 kHz now.
        rate: if d.rate == 0 { 48_000 } else { d.rate },
        bits: d.bits,
        mix: d.mix,
        focused: false,
    });
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        vec2(104.0, 20.0),
        egui::Label::new(RichText::new(text)).halign(egui::Align::Max),
    );
}

fn rate_box(ui: &mut egui::Ui, id: &str, rate: &mut u32) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{rate}"))
        .width(160.0)
        .show_ui(ui, |ui| {
            for r in RATES {
                ui.selectable_value(rate, r, format!("{r}"));
            }
        });
    ui.label("Hz");
}

fn bits_box(ui: &mut egui::Ui, id: &str, bits: &mut u16) {
    let name = |b: u16| match b {
        32 => "32 (float)".to_string(),
        b => b.to_string(),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(name(*bits))
        .width(160.0)
        .show_ui(ui, |ui| {
            for b in [32u16, 24, 16] {
                ui.selectable_value(bits, b, name(b));
            }
        });
    ui.label("bits");
}

fn channels_name(n: usize) -> &'static str {
    match n {
        1 => "Mono",
        2 => "Stereo",
        6 => "5.1",
        8 => "7.1",
        _ => "Stereo",
    }
}

/// Both dialogs, when open.
pub fn show(ctx: &egui::Context, st: &mut State) {
    new_file_window(ctx, st);
    new_session_window(ctx, st);
}

fn buttons(ui: &mut egui::Ui, th: &crate::theme::Theme, enter: bool) -> (bool, bool) {
    let mut ok = false;
    let mut cancel = false;
    ui.add_space(8.0);
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(w, 26.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if ui.button("  Cancel  ").clicked() {
                cancel = true;
            }
            let b = egui::Button::new(RichText::new("    OK    ").strong().color(c(th.background)))
                .fill(c(th.accent));
            if ui.add(b).clicked() || enter {
                ok = true;
            }
        },
    );
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    (ok, cancel)
}

fn new_file_window(ctx: &egui::Context, st: &mut State) {
    let Some(mut d) = st.new_file_ui.take() else {
        return;
    };
    let th = st.theme.clone();
    let mut open = true;
    let mut result = None;
    crate::ui::popup(
        ctx,
        egui::Id::new(("af2-popup", "new_dialogs.rs", 0)),
        if d.record_after { "New Audio File to Record" } else { "New Audio File" },
    )
    .open(&mut open)
    .collapsible(false)
    .resizable(false)
    .pivot(Align2::CENTER_CENTER)
    .default_pos(ctx.screen_rect().center())
    .show(ctx, |ui| {
        let mut enter = false;
        egui::Grid::new("new-file-grid")
            .num_columns(2)
            .spacing(vec2(10.0, 8.0))
            .show(ui, |ui| {
                label(ui, "File Name:");
                let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(200.0));
                if !d.focused {
                    r.request_focus();
                    d.focused = true;
                }
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.end_row();
                label(ui, "Sample Rate:");
                ui.horizontal(|ui| rate_box(ui, "nf-rate", &mut d.rate));
                ui.end_row();
                label(ui, "Channels:");
                egui::ComboBox::from_id_salt("nf-ch")
                    .selected_text(channels_name(d.channels))
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for n in [1usize, 2, 6, 8] {
                            ui.selectable_value(&mut d.channels, n, channels_name(n));
                        }
                    });
                ui.end_row();
                label(ui, "Bit Depth:");
                ui.horizontal(|ui| bits_box(ui, "nf-bits", &mut d.bits));
                ui.end_row();
            });
        ui.checkbox(&mut d.make_default, "Make these my defaults")
            .on_hover_text("Unticked, new files start at 48 kHz, stereo, 24-bit");
        ui.label(
            RichText::new("Saved as FLAC by default (lossless and compressed).")
                .size(10.5)
                .color(c(th.text_dim)),
        );
        let (ok, cancel) = buttons(ui, &th, enter);
        if ok {
            result = Some(true);
        } else if cancel {
            result = Some(false);
        }
    });
    match (result, open) {
        (Some(true), _) => {
            let name = if d.name.trim().is_empty() {
                format!("Untitled {}", st.docs.len() + 1)
            } else {
                d.name.trim().to_string()
            };
            st.new_file(d.rate, d.channels);
            if let Some(doc) = st.doc_mut() {
                doc.name = name;
                doc.new_bits = Some(d.bits);
            }
            st.prefs.new_file = if d.make_default {
                NewFileDefaults {
                    rate: d.rate,
                    channels: d.channels,
                    bits: d.bits,
                    chosen: true,
                }
            } else {
                NewFileDefaults::default()
            };
            st.prefs.save();
            if d.record_after {
                st.start_recording();
            }
        }
        (Some(false), _) | (None, false) => {}
        (None, true) => st.new_file_ui = Some(d),
    }
}

fn new_session_window(ctx: &egui::Context, st: &mut State) {
    let Some(mut d) = st.new_session_ui.take() else {
        return;
    };
    let th = st.theme.clone();
    let mut open = true;
    let mut result = None;
    let templates = crate::sessions::list_templates();
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "new_dialogs.rs", 1)), "New Multitrack Session")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center())
        .show(ctx, |ui| {
            let mut enter = false;
            egui::Grid::new("new-session-grid")
                .num_columns(2)
                .spacing(vec2(10.0, 8.0))
                .show(ui, |ui| {
                    label(ui, "Session Name:");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(300.0));
                    if !d.focused {
                        r.request_focus();
                        d.focused = true;
                    }
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    label(ui, "Folder Location:");
                    ui.horizontal(|ui| {
                        let mut text = d.folder.display().to_string();
                        if ui
                            .add(egui::TextEdit::singleline(&mut text).desired_width(220.0))
                            .changed()
                        {
                            d.folder = PathBuf::from(text);
                        }
                        if ui.button("Browse...").clicked() {
                            if let Some(p) = crate::file_dialog::FileDialog::new()
                                .set_directory(&d.folder)
                                .pick_folder()
                            {
                                d.folder = p;
                            }
                        }
                    });
                    ui.end_row();
                    label(ui, "Template:");
                    let tname = |p: &Option<PathBuf>| {
                        p.as_ref()
                            .and_then(|p| p.file_stem())
                            .map_or("None".to_string(), |s| s.to_string_lossy().to_string())
                    };
                    egui::ComboBox::from_id_salt("ns-template")
                        .selected_text(tname(&d.template))
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.template, None, "None");
                            for t in &templates {
                                let some = Some(t.clone());
                                let label = tname(&some);
                                ui.selectable_value(&mut d.template, some, label);
                            }
                        });
                    ui.end_row();
                    label(ui, "Sample Rate:");
                    ui.horizontal(|ui| rate_box(ui, "ns-rate", &mut d.rate));
                    ui.end_row();
                    label(ui, "Bit Depth:");
                    ui.horizontal(|ui| bits_box(ui, "ns-bits", &mut d.bits));
                    ui.end_row();
                    label(ui, "Mix:");
                    let mix_name = |m: &str| match m {
                        "2.0" => "Stereo".to_string(),
                        other => format!("{other} (3D Space)"),
                    };
                    egui::ComboBox::from_id_salt("ns-mix")
                        .selected_text(mix_name(&d.mix))
                        .width(160.0)
                        .show_ui(ui, |ui| {
                            for l in SpaceLayout::ALL {
                                let key = l.label().to_string();
                                let shown = mix_name(&key);
                                ui.selectable_value(&mut d.mix, key, shown);
                            }
                        });
                    ui.end_row();
                });
            if d.template.is_some() {
                ui.label(
                    RichText::new("The template's tracks and settings are used; its sample rate is replaced by the one above.")
                        .size(10.5)
                        .color(c(th.text_dim)),
                );
            }
            let target = d.folder.join(d.name.trim());
            ui.label(
                RichText::new(format!("Saved in {}", target.display()))
                    .size(10.5)
                    .color(c(th.text_dim)),
            );
            let (ok, cancel) = buttons(ui, &th, enter);
            if ok {
                result = Some(true);
            } else if cancel {
                result = Some(false);
            }
        });
    match (result, open) {
        (Some(true), _) => create_session(st, d),
        (Some(false), _) | (None, false) => {}
        (None, true) => st.new_session_ui = Some(d),
    }
}

fn clean_name(s: &str) -> String {
    let t: String = s
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if t.is_empty() {
        "Untitled Session".into()
    } else {
        t
    }
}

/// Makes the session: its folder and file on disk right away, as the
/// leading editors do, so recordings and imports land beside it.
fn create_session(st: &mut State, d: NewSessionDraft) {
    let name = clean_name(&d.name);
    st.new_session(d.template.as_deref());
    st.session.name = name.clone();
    st.session.rate = d.rate;
    st.session.bit_depth = d.bits;
    st.session.space = SpaceLayout::ALL
        .into_iter()
        .find(|l| l.label() == d.mix && *l != SpaceLayout::Stereo20);
    st.session_version += 1;
    st.prefs.new_session = NewSessionDefaults {
        folder: Some(d.folder.clone()),
        rate: d.rate,
        bits: d.bits,
        mix: d.mix.clone(),
    };
    st.prefs.save();
    let dir = d.folder.join(&name);
    match std::fs::create_dir_all(&dir) {
        Ok(()) => st.save_session_to(dir.join(format!("{name}.audfrm"))),
        Err(e) => st.error(format!(
            "Could not make the session folder {}: {e}. The session is open but not saved yet.",
            dir.display()
        )),
    }
}

/// The extension new audio from the app is written with: FLAC (lossless
/// and compressed), or 32-bit float WAV when the audio goes past full
/// scale, which FLAC's whole numbers would clip.
pub fn lossless_ext(buf: &af2_core::AudioBuffer) -> &'static str {
    let over = buf
        .channels
        .iter()
        .any(|ch| ch.iter().any(|s| s.abs() > 1.0));
    if over {
        "wav"
    } else {
        "flac"
    }
}

/// Writes `buf` by its path's extension: a `.wav` (also `.wav.part`) as
/// 32-bit float, anything else as FLAC at the smallest size (level 8),
/// 16-bit for a 16-bit choice, else 24-bit (FLAC holds no float).
pub fn write_lossless(
    path: &std::path::Path,
    buf: &af2_core::AudioBuffer,
    bits: u16,
) -> Result<(), String> {
    use af2_io::export::{Container, Options, SampleFormat};
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".wav") || name.ends_with(".wav.part") {
        return af2_io::write_wav(path, buf, af2_io::WavFormat::Float32);
    }
    let opts = Options {
        container: Container::Flac,
        format: if bits == 16 {
            SampleFormat::I16
        } else {
            SampleFormat::I24
        },
        dither: bits == 16,
        flac_level: 8,
        ..Options::default()
    };
    af2_io::export::export_buffer(path, buf, &opts, &Default::default()).map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn recordings_write_as_flac_and_read_back() {
        let dir = std::env::temp_dir().join(format!("af2-flac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut b = af2_core::AudioBuffer::new(48_000, 2, 4800);
        for (i, s) in b.channels[0].iter_mut().enumerate() {
            *s = (i as f32 * 0.05).sin() * 0.5;
        }
        for bits in [16u16, 24, 32] {
            let p = dir.join(format!("t{bits}.flac"));
            super::write_lossless(&p, &b, bits).unwrap();
            let back = af2_io::decode_file(&p).unwrap();
            assert_eq!(back.frames(), 4800);
            assert_eq!(back.num_channels(), 2);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_past_full_scale_goes_to_float_wav() {
        let mut b = af2_core::AudioBuffer::new(48_000, 1, 10);
        assert_eq!(super::lossless_ext(&b), "flac");
        b.channels[0][3] = 1.5;
        assert_eq!(super::lossless_ext(&b), "wav");
        let dir = std::env::temp_dir().join(format!("af2-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("hot.wav");
        super::write_lossless(&p, &b, 24).unwrap();
        let back = af2_io::decode_file(&p).unwrap();
        assert!(
            (back.channels[0][3] - 1.5).abs() < 1e-6,
            "kept above full scale"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_names_never_empty_or_unsafe() {
        assert_eq!(super::clean_name("  a/b:c "), "a_b_c");
        assert_eq!(super::clean_name("   "), "Untitled Session");
    }
}

#[cfg(test)]
mod defaults_tests {
    use super::*;

    #[test]
    fn old_remembered_choices_give_way_to_48k_24_bit() {
        // What an older copy saved: the last file made, 44.1 kHz 32-bit.
        let old: NewFileDefaults =
            serde_json::from_str(r#"{"rate":44100,"channels":2,"bits":32}"#).unwrap();
        let s = old.starting();
        assert_eq!((s.rate, s.channels, s.bits), (48_000, 2, 24));
        // Ticked "Make these my defaults": they stay.
        let mine = NewFileDefaults { rate: 96_000, channels: 1, bits: 32, chosen: true };
        assert_eq!(mine.starting().rate, 96_000);
    }
}
