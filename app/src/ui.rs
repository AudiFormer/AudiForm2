// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

//! The window: menu bar, tool strip, the dockable panel area, transport
//! and status bar. Every panel lives in the dock and can be dragged to a
//! new place, tabbed with others, split, or floated in its own window.

use egui::{Align2, FontId, Key, KeyboardShortcut, Modifiers, RichText, Rounding, Sense, Vec2};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use serde::{Deserialize, Serialize};

use crate::state::{data_dir, Mode, State, Tool};
use crate::theme::c;
use af27_core::time::{format_time, gain_to_db};
use af27_dsp::effects::GROUPS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    Files,
    Markers,
    Effects,
    AiSeparation,
    History,
    Editor,
    Mixer,
    Levels,
    Phase,
    Statistics,
    Spectrum,
    Selection,
}

impl Tab {
    pub const ALL: [Tab; 12] = [
        Tab::Editor,
        Tab::Mixer,
        Tab::Files,
        Tab::Markers,
        Tab::Effects,
        Tab::AiSeparation,
        Tab::History,
        Tab::Levels,
        Tab::Phase,
        Tab::Statistics,
        Tab::Spectrum,
        Tab::Selection,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Markers => "Markers",
            Tab::Effects => "Effects",
            Tab::AiSeparation => "AI Stems",
            Tab::History => "History",
            Tab::Editor => "Editor",
            Tab::Mixer => "Mixer",
            Tab::Levels => "Levels",
            Tab::Phase => "Phase Scope",
            Tab::Statistics => "Level Statistics",
            Tab::Spectrum => "Spectrum",
            Tab::Selection => "Range",
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Editor, Tab::Mixer]);
    let s = dock.main_surface_mut();
    let [center, left] = s.split_left(NodeIndex::root(), 0.16, vec![Tab::Files, Tab::Markers]);
    let [_files, left_mid] = s.split_below(left, 0.42, vec![Tab::Effects, Tab::AiSeparation]);
    let [_fx, _hist] = s.split_below(left_mid, 0.62, vec![Tab::History]);
    let [center, _levels] = s.split_below(center, 0.9, vec![Tab::Levels]);
    let [_editor, right] = s.split_right(center, 0.83, vec![Tab::Phase]);
    let [_phase, right_mid] = s.split_below(right, 0.38, vec![Tab::Statistics, Tab::Spectrum]);
    let [_stats, _range] = s.split_below(right_mid, 0.7, vec![Tab::Selection]);
    dock
}

pub struct App {
    pub st: State,
    dock: DockState<Tab>,
    pub viewer: Caches,
}

/// What a cached spectrogram was drawn from.
pub type SpectroKey = (u64, i64, i64, usize, usize, String);

/// Per-panel drawing caches (textures and the like) kept between frames.
#[derive(Default)]
pub struct Caches {
    pub spectro: Option<(egui::TextureHandle, SpectroKey)>,
    pub drag: crate::multitrack::Drag,
    pub wave_drag: Option<crate::waveform::WaveDrag>,
    pub marker_edit: Option<usize>,
}

fn layout_path() -> std::path::PathBuf {
    data_dir().join("layout.json")
}

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<std::path::PathBuf>) -> App {
        let mut st = State::new();
        st.theme.apply(&cc.egui_ctx);
        st.theme_changed = false;
        st.open_paths(files);
        let dock = std::fs::read_to_string(layout_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(default_layout);
        App {
            st,
            dock,
            viewer: Caches::default(),
        }
    }

    fn show_tab(&mut self, tab: Tab) {
        if let Some(found) = self.dock.find_tab(&tab) {
            self.dock.set_active_tab(found);
        } else {
            self.dock.add_window(vec![tab]);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string(&self.dock) {
            let _ = std::fs::write(layout_path(), s);
        }
    }
}

struct Viewer<'a> {
    st: &'a mut State,
    caches: &'a mut Caches,
    open_tab: &'a mut Option<Tab>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let t = tab.title();
        if *tab == Tab::Editor {
            let name = match self.st.mode {
                Mode::Waveform => self
                    .st
                    .doc()
                    .map(|d| format!("{}{}", d.name, if d.dirty { " *" } else { "" }))
                    .unwrap_or_else(|| "no file".into()),
                Mode::Multitrack => format!(
                    "{}{}",
                    self.st.session.name,
                    if self.st.session_dirty { " *" } else { "" }
                ),
            };
            return format!("{t}: {name}").into();
        }
        t.into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Editor => match self.st.mode {
                Mode::Waveform => crate::waveform::editor(ui, self.st, self.caches),
                Mode::Multitrack => crate::multitrack::editor(ui, self.st, self.caches),
            },
            Tab::Mixer => crate::multitrack::mixer(ui, self.st),
            Tab::Files => crate::panels::files(ui, self.st, self.open_tab),
            Tab::Markers => crate::panels::markers(ui, self.st, self.caches),
            Tab::Effects => crate::panels::effects(ui, self.st),
            Tab::AiSeparation => crate::panels::separation(ui, self.st),
            Tab::History => crate::panels::history(ui, self.st),
            Tab::Levels => crate::panels::levels(ui, self.st),
            Tab::Phase => crate::panels::phase(ui, self.st),
            Tab::Statistics => crate::panels::statistics(ui, self.st),
            Tab::Spectrum => crate::panels::spectrum(ui, self.st),
            Tab::Selection => crate::panels::range(ui, self.st),
        }
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool {
        true
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Editor | Tab::Levels | Tab::Phase | Tab::Spectrum => [false, false],
            _ => [false, true],
        }
    }
}

fn shortcut(ctx: &egui::Context, m: Modifiers, k: Key) -> bool {
    ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
}

impl App {
    fn keys(&mut self, ctx: &egui::Context) {
        let st = &mut self.st;
        let cmd = Modifiers::COMMAND;
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        if shortcut(ctx, cs, Key::Z) || shortcut(ctx, cmd, Key::Y) {
            st.redo();
        }
        if shortcut(ctx, cmd, Key::Z) {
            st.undo();
        }
        if shortcut(ctx, cmd, Key::O) {
            open_dialog(st);
        }
        if shortcut(ctx, cmd, Key::S) {
            save(st);
        }
        if shortcut(ctx, cmd, Key::N) {
            st.new_file(48_000, 2);
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if shortcut(ctx, cmd, Key::A) {
            st.select_all();
        }
        if shortcut(ctx, cmd, Key::C) {
            st.edit_copy();
        }
        if shortcut(ctx, cmd, Key::X) {
            st.edit_cut();
        }
        if shortcut(ctx, cmd, Key::V) {
            st.edit_paste();
        }
        if shortcut(ctx, cmd, Key::T) {
            st.edit_crop();
        }
        if shortcut(ctx, cmd, Key::K) {
            st.split_at_playhead();
        }
        if shortcut(ctx, cmd, Key::L) {
            st.looping = !st.looping;
        }
        if shortcut(ctx, Modifiers::SHIFT, Key::Space) {
            if st.is_recording() {
                st.stop_recording();
            } else {
                st.start_recording();
            }
        }
        let plain = |k| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if plain(Key::Space) {
            st.toggle_play();
        }
        if plain(Key::Delete) || plain(Key::Backspace) {
            st.edit_delete("Delete");
        }
        if plain(Key::Home) {
            st.set_cursor(0.0);
        }
        if plain(Key::End) {
            let l = st.program_len();
            st.set_cursor(l);
        }
        if plain(Key::M) {
            st.add_marker();
        }
        if plain(Key::Num9) {
            st.mode = Mode::Waveform;
        }
        if plain(Key::Num0) {
            st.mode = Mode::Multitrack;
        }
        if plain(Key::V) {
            st.tool = Tool::Move;
        }
        if plain(Key::R) {
            st.tool = Tool::Razor;
        }
        if plain(Key::T) {
            st.tool = Tool::TimeSelect;
        }
        if plain(Key::Equals) || plain(Key::Plus) {
            zoom(st, 0.5);
        }
        if plain(Key::Minus) {
            zoom(st, 2.0);
        }
        if plain(Key::Escape) {
            match st.mode {
                Mode::Waveform => {
                    if let Some(d) = st.doc_mut() {
                        d.sel = None;
                    }
                }
                Mode::Multitrack => {
                    st.session_sel = None;
                    st.selected_clip = None;
                }
            }
        }
    }
}

pub fn zoom(st: &mut State, f: f64) {
    let anchor = st.position();
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                let dur = d.duration();
                d.view.zoom(f, anchor, dur);
            }
        }
        Mode::Multitrack => {
            let dur = st.session.duration().max(30.0);
            st.session_view.zoom(f, anchor, dur);
        }
    }
}

fn zoom_full(st: &mut State) {
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                d.view = crate::state::View::fit(d.duration());
            }
        }
        Mode::Multitrack => {
            st.session_view = crate::state::View::fit(st.session.duration().max(10.0))
        }
    }
}

const AUDIO_FILTER: &[&str] = af27_io::EXTENSIONS;

pub fn open_dialog(st: &mut State) {
    let mut all: Vec<&str> = AUDIO_FILTER.to_vec();
    all.extend(["af27", "sesx", "audfrm", "altaudio"]);
    if let Some(paths) = rfd::FileDialog::new()
        .add_filter("Audio and sessions", &all)
        .add_filter("Audio", AUDIO_FILTER)
        .add_filter("Sessions", &["af27", "sesx", "audfrm", "altaudio"])
        .pick_files()
    {
        st.open_paths(paths);
    }
}

fn save(st: &mut State) {
    match st.mode {
        Mode::Waveform => {
            let wav_path = st
                .doc()
                .and_then(|d| d.path.clone())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")));
            match wav_path {
                Some(p) => st.save_doc_as(p, af27_io::WavFormat::Float32),
                None => save_doc_as(st),
            }
        }
        Mode::Multitrack => match st.session_path.clone() {
            Some(p) => st.save_session_to(p),
            None => save_session_as(st),
        },
    }
}

fn save_doc_as(st: &mut State) {
    let Some(d) = st.doc() else { return };
    let stem = d
        .name
        .rsplit_once('.')
        .map_or(d.name.as_str(), |(a, _)| a)
        .to_string();
    if let Some(p) = rfd::FileDialog::new()
        .set_file_name(format!("{stem}.wav"))
        .add_filter("WAV (32-bit float)", &["wav"])
        .save_file()
    {
        st.save_doc_as(p, af27_io::WavFormat::Float32);
    }
}

fn save_session_as(st: &mut State) {
    if let Some(p) = rfd::FileDialog::new()
        .set_file_name(format!("{}.af27", st.session.name))
        .add_filter("Audiform27 session", &["af27"])
        .save_file()
    {
        st.save_session_to(p);
    }
}

fn menus(app: &mut App, ui: &mut egui::Ui) {
    let st = &mut app.st;
    let mut open_tab = None;
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("New Audio File          Ctrl+N").clicked() {
                st.new_file(48_000, 2);
                ui.close_menu();
            }
            if ui.button("New Multitrack Session").clicked() {
                let before = st.session.clone();
                st.session = af27_core::Session::default();
                st.session_history.push("New session", before);
                st.session_path = None;
                st.media_docs.clear();
                st.session_version += 1;
                st.mode = Mode::Multitrack;
                ui.close_menu();
            }
            if ui.button("Open...                       Ctrl+O").clicked() {
                open_dialog(st);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Save                          Ctrl+S").clicked() {
                save(st);
                ui.close_menu();
            }
            if ui.button("Save Audio As...").clicked() {
                save_doc_as(st);
                ui.close_menu();
            }
            ui.menu_button("Save Audio As Format", |ui| {
                for fmt in af27_io::WavFormat::ALL {
                    if ui.button(format!("WAV, {}", fmt.label())).clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("WAV", &["wav"])
                            .save_file()
                        {
                            st.save_doc_as(p, fmt);
                        }
                        ui.close_menu();
                    }
                }
            });
            if ui.button("Save Session As...").clicked() {
                save_session_as(st);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Export Multitrack Mixdown...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .set_file_name(format!("{} Mixdown.wav", st.session.name))
                    .add_filter("WAV", &["wav"])
                    .save_file()
                {
                    st.export_mixdown(p, af27_io::WavFormat::Pcm24);
                }
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Close File").clicked() {
                if let Some(id) = st.current {
                    st.close_doc(id);
                }
                ui.close_menu();
            }
            if ui.button("Exit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            let ul = match st.mode {
                Mode::Waveform => st
                    .doc()
                    .and_then(|d| d.history.undo_label().map(str::to_string)),
                Mode::Multitrack => st.session_history.undo_label().map(str::to_string),
            };
            if ui
                .add_enabled(
                    ul.is_some(),
                    egui::Button::new(format!("Undo {}    Ctrl+Z", ul.unwrap_or_default())),
                )
                .clicked()
            {
                st.undo();
                ui.close_menu();
            }
            if ui.button("Redo                Ctrl+Y").clicked() {
                st.redo();
                ui.close_menu();
            }
            ui.separator();
            for (label, f) in [
                ("Cut              Ctrl+X", State::edit_cut as fn(&mut State)),
                ("Copy            Ctrl+C", State::edit_copy),
                ("Paste           Ctrl+V", State::edit_paste),
                ("Crop            Ctrl+T", State::edit_crop),
                ("Select All    Ctrl+A", State::select_all),
                ("Add Marker   M", State::add_marker),
            ] {
                if ui.button(label).clicked() {
                    f(st);
                    ui.close_menu();
                }
            }
            if ui.button("Delete          Del").clicked() {
                st.edit_delete("Delete");
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Insert Silence (5 s)").clicked() {
                let (rate, ch) = st
                    .doc()
                    .map_or((48_000, 2), |d| (d.buf.rate, d.buf.num_channels()));
                st.insert_buffer("Insert silence", af27_dsp::generate::silence(rate, ch, 5.0));
                ui.close_menu();
            }
        });
        ui.menu_button("Multitrack", |ui| {
            if ui.button("Add Track").clicked() {
                st.add_track();
                ui.close_menu();
            }
            if ui.button("Split at Playhead     Ctrl+K").clicked() {
                st.split_at_playhead();
                ui.close_menu();
            }
            if ui.button("Delete Selected Clip").clicked() {
                st.delete_clip();
                ui.close_menu();
            }
            if ui.button("Insert Current File at Playhead").clicked() {
                if let (Some(d), Some(t)) = (
                    st.current,
                    st.selected_track
                        .or(st.session.tracks.first().map(|t| t.id)),
                ) {
                    let at = st.session_cursor;
                    st.place_doc(d, t, at);
                }
                ui.close_menu();
            }
        });
        ui.menu_button("Effects", |ui| {
            for (group, list) in GROUPS {
                ui.menu_button(*group, |ui| {
                    for e in *list {
                        if ui.button(e.name()).clicked() {
                            st.effect = *e;
                            open_tab = Some(Tab::Effects);
                            ui.close_menu();
                        }
                    }
                });
            }
            ui.separator();
            if ui.button("Generate Tone or Noise...").clicked() {
                st.generate.open = true;
                ui.close_menu();
            }
            if ui.button("AI Stem Separation...").clicked() {
                open_tab = Some(Tab::AiSeparation);
                ui.close_menu();
            }
        });
        ui.menu_button("View", |ui| {
            if ui
                .radio(st.mode == Mode::Waveform, "Waveform Editor   9")
                .clicked()
            {
                st.mode = Mode::Waveform;
                ui.close_menu();
            }
            if ui
                .radio(st.mode == Mode::Multitrack, "Multitrack Editor   0")
                .clicked()
            {
                st.mode = Mode::Multitrack;
                ui.close_menu();
            }
            ui.separator();
            if let Some(d) = st.doc_mut() {
                ui.checkbox(&mut d.spectral, "Spectral Display");
            }
            if ui.button("Zoom In   =").clicked() {
                zoom(st, 0.5);
            }
            if ui.button("Zoom Out   -").clicked() {
                zoom(st, 2.0);
            }
            if ui.button("Zoom Out Full").clicked() {
                zoom_full(st);
            }
            ui.checkbox(&mut st.return_on_stop, "Return Playhead on Stop");
            ui.separator();
            ui.menu_button("Theme", |ui| {
                let names: Vec<String> = st.themes.iter().map(|t| t.name.clone()).collect();
                for (i, n) in names.iter().enumerate() {
                    if ui.radio(st.theme.name == *n, n).clicked() {
                        st.theme = st.themes[i].clone();
                        st.theme_changed = true;
                        let _ = std::fs::create_dir_all(data_dir());
                        let _ = std::fs::write(data_dir().join("theme.txt"), n);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Save Current Theme As...").clicked() {
                    let dir = data_dir().join("themes");
                    let _ = std::fs::create_dir_all(&dir);
                    if let Some(p) = rfd::FileDialog::new()
                        .set_directory(&dir)
                        .set_file_name(format!("{}.af27theme", st.theme.name))
                        .add_filter("Audiform27 theme", &["af27theme"])
                        .save_file()
                    {
                        let mut t = st.theme.clone();
                        t.name = p
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or(t.name);
                        let _ = std::fs::write(&p, serde_json::to_string_pretty(&t).unwrap());
                        st.themes = crate::theme::load_all(&dir);
                    }
                    ui.close_menu();
                }
                if ui.button("Reload Themes").clicked() {
                    st.themes = crate::theme::load_all(&data_dir().join("themes"));
                    ui.close_menu();
                }
                ui.add(egui::Slider::new(&mut st.theme.glow, 0.0..=2.0).text("Glow"));
                if ui
                    .add(egui::Slider::new(&mut st.theme.rounding, 0.0..=14.0).text("Rounding"))
                    .changed()
                {
                    st.theme_changed = true;
                }
                if ui
                    .add(egui::Slider::new(&mut st.theme.font_scale, 0.8..=1.4).text("Text size"))
                    .changed()
                {
                    st.theme_changed = true;
                }
            });
        });
        ui.menu_button("Window", |ui| {
            for t in Tab::ALL {
                if ui.button(t.title()).clicked() {
                    open_tab = Some(t);
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui.button("Reset Layout").clicked() {
                app.dock = default_layout();
                ui.close_menu();
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("About Audiform27").clicked() {
                st.show_about = true;
                ui.close_menu();
            }
        });
    });
    if let Some(t) = open_tab {
        app.show_tab(t);
    }
}

fn mode_button(ui: &mut egui::Ui, st: &mut State, mode: Mode, label: &str) {
    let on = st.mode == mode;
    let th = &st.theme;
    let text = RichText::new(label)
        .strong()
        .color(if on { c(th.background) } else { c(th.text) });
    let b = egui::Button::new(text)
        .fill(if on { c(th.accent) } else { c(th.panel_raised) })
        .rounding(Rounding::same(th.rounding));
    let r = ui.add(b);
    if on {
        th.glow_rect(ui.painter(), r.rect, th.accent, 1.0);
    }
    if r.clicked() {
        st.mode = mode;
    }
}

fn toolbar(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        mode_button(ui, st, Mode::Waveform, "🎵 Waveform");
        mode_button(ui, st, Mode::Multitrack, "☰ Multitrack");
        ui.add_space(14.0);
        for (tool, label, tip) in [
            (Tool::Move, "↔", "Move (V)"),
            (Tool::Razor, "✂", "Razor (R)"),
            (Tool::TimeSelect, "I", "Time Selection (T)"),
        ] {
            if ui
                .selectable_label(st.tool == tool, RichText::new(label).size(16.0))
                .on_hover_text(tip)
                .clicked()
            {
                st.tool = tool;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            let wordmark = RichText::new("AUDIFORM27")
                .strong()
                .size(15.0)
                .color(c(st.theme.accent));
            ui.label(wordmark);
        });
    });
}

fn transport(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    ui.horizontal(|ui| {
        // Big timecode.
        let t = format_time(st.position());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(190.0, 34.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, Rounding::same(th.rounding), c(th.wave_background));
        th.glow_rect(p, rect, th.accent, 0.5);
        p.text(
            rect.left_center() + egui::vec2(12.0, 0.0),
            Align2::LEFT_CENTER,
            t,
            FontId::monospace(22.0),
            c(th.accent),
        );
        ui.add_space(ui.available_width() / 2.0 - 190.0);
        let btn = |ui: &mut egui::Ui, s: &str, tip: &str, on: bool, col: [u8; 3]| {
            let b = egui::Button::new(RichText::new(s).size(18.0).color(if on {
                c(th.background)
            } else {
                c(col)
            }))
            .fill(if on { c(col) } else { c(th.panel_raised) })
            .rounding(Rounding::same(th.rounding))
            .min_size(Vec2::new(34.0, 30.0));
            let r = ui.add(b).on_hover_text(tip);
            if on {
                th.glow_rect(ui.painter(), r.rect, col, 1.2);
            }
            r.clicked()
        };
        let playing = st.is_playing();
        let rec = st.is_recording();
        if btn(ui, "⏹", "Stop", false, th.text) {
            if rec {
                st.stop_recording();
            } else {
                st.stop();
            }
        }
        if btn(
            ui,
            if playing { "⏸" } else { "▶" },
            "Play / Stop (Space)",
            playing,
            th.accent,
        ) {
            st.toggle_play();
        }
        if btn(ui, "⏮", "Go to Start (Home)", false, th.text) {
            st.set_cursor(0.0);
        }
        if btn(ui, "⏭", "Go to End (End)", false, th.text) {
            let l = st.program_len();
            st.set_cursor(l);
        }
        if btn(ui, "⏺", "Record (Shift+Space)", rec, th.accent_alt) {
            if rec {
                st.stop_recording();
            } else {
                st.start_recording();
            }
        }
        let lp = st.looping;
        if btn(ui, "🔁", "Loop Playback (Ctrl+L)", lp, th.accent) {
            st.looping = !st.looping;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (s, tip, f) in [
                ("⛶", "Zoom Out Full", 0.0),
                ("−", "Zoom Out (-)", 2.0),
                ("+", "Zoom In (=)", 0.5),
            ] {
                if ui
                    .button(RichText::new(s).size(16.0))
                    .on_hover_text(tip)
                    .clicked()
                {
                    if f == 0.0 {
                        zoom_full(st);
                    } else {
                        zoom(st, f);
                    }
                }
            }
        });
    });
}

fn status(ui: &mut egui::Ui, st: &State) {
    ui.horizontal(|ui| {
        let th = &st.theme;
        if let Some(t) = st.toasts.last() {
            let col = if t.error {
                c(th.accent_alt)
            } else {
                c(th.text)
            };
            ui.label(RichText::new(&t.text).color(col));
        } else if !st.loading().is_empty() {
            ui.spinner();
            ui.label(format!("Opening {}", st.loading().join(", ")));
        } else if let Some(e) = &st.engine_error {
            ui.label(RichText::new(e).color(c(th.accent_alt)));
        } else {
            ui.label(RichText::new("Ready").color(c(th.text_dim)));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let dim = c(th.text_dim);
            if let Some(e) = &st.engine {
                ui.label(RichText::new(format!("Out: {} @ {} Hz", e.device, e.rate)).color(dim));
            }
            ui.separator();
            match st.mode {
                Mode::Waveform => {
                    if let Some(d) = st.doc() {
                        let ch = match d.buf.num_channels() {
                            1 => "Mono".to_string(),
                            2 => "Stereo".to_string(),
                            6 => "5.1".to_string(),
                            8 => "7.1".to_string(),
                            n => format!("{n} ch"),
                        };
                        let mb = (d.buf.frames() * d.buf.num_channels() * 4) as f64 / 1_048_576.0;
                        ui.label(
                            RichText::new(format!(
                                "{} Hz • 32-bit float • {ch} • {} • {mb:.1} MB",
                                d.buf.rate,
                                format_time(d.duration())
                            ))
                            .color(dim),
                        );
                    }
                }
                Mode::Multitrack => {
                    ui.label(
                        RichText::new(format!(
                            "{} Hz session • {} tracks • {}",
                            st.session.rate,
                            st.session.tracks.len(),
                            format_time(st.session.duration())
                        ))
                        .color(dim),
                    );
                }
            }
        });
    });
}

fn generate_window(ctx: &egui::Context, st: &mut State) {
    let mut open = st.generate.open;
    egui::Window::new("Generate")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let g = &mut st.generate;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut g.kind, 0, "Tone");
                ui.selectable_value(&mut g.kind, 1, "Noise");
                ui.selectable_value(&mut g.kind, 2, "Silence");
            });
            if g.kind == 0 {
                ui.horizontal(|ui| {
                    for (i, n) in ["Sine", "Square", "Triangle", "Saw"].iter().enumerate() {
                        ui.selectable_value(&mut g.wave, i, *n);
                    }
                });
                ui.add(
                    egui::Slider::new(&mut g.freq, 20.0..=20_000.0)
                        .logarithmic(true)
                        .suffix(" Hz")
                        .text("Frequency"),
                );
            }
            if g.kind == 1 {
                ui.horizontal(|ui| {
                    for (i, n) in ["White", "Pink", "Brown"].iter().enumerate() {
                        ui.selectable_value(&mut g.noise, i, *n);
                    }
                });
            }
            if g.kind != 2 {
                ui.add(
                    egui::Slider::new(&mut g.db, -60.0..=0.0)
                        .suffix(" dB")
                        .text("Level"),
                );
            }
            ui.add(
                egui::Slider::new(&mut g.secs, 0.1..=600.0)
                    .logarithmic(true)
                    .suffix(" s")
                    .text("Duration"),
            );
            if ui.button("Insert at Playhead").clicked() {
                let (rate, ch) = st
                    .doc()
                    .map_or((48_000, 2), |d| (d.buf.rate, d.buf.num_channels()));
                let g = &st.generate;
                use af27_dsp::generate as gen;
                let buf = match g.kind {
                    0 => gen::tone(
                        rate,
                        ch,
                        g.secs,
                        g.freq,
                        g.db,
                        [
                            gen::Wave::Sine,
                            gen::Wave::Square,
                            gen::Wave::Triangle,
                            gen::Wave::Saw,
                        ][g.wave],
                    ),
                    1 => gen::noise(
                        rate,
                        ch,
                        g.secs,
                        g.db,
                        [gen::Noise::White, gen::Noise::Pink, gen::Noise::Brown][g.noise],
                    ),
                    _ => gen::silence(rate, ch, g.secs),
                };
                st.mode = Mode::Waveform;
                st.insert_buffer("Generate", buf);
            }
        });
    st.generate.open = open && st.generate.open;
}

fn about_window(ctx: &egui::Context, st: &mut State) {
    let mut open = st.show_about;
    egui::Window::new("About Audiform27").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(RichText::new("AUDIFORM27").size(28.0).strong().color(c(st.theme.accent)));
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label("A fast, themeable audio editor written entirely in Rust.");
        ui.label("Licensed MIT OR Apache-2.0. The DTS decoder (af27-dts) is LGPL-2.1-or-later, ported from FFmpeg.");
        ui.label("The sequel to Audiform, by the same project lead.");
    });
    st.show_about = open;
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.st.theme_changed {
            self.st.theme.apply(ctx);
            self.st.theme_changed = false;
        }
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.st.open_paths(dropped);
        }
        self.st.tick();
        self.keys(ctx);
        let th = self.st.theme.clone();
        let frame = |fill: [u8; 3]| {
            egui::Frame::none()
                .fill(c(fill))
                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        };

        egui::TopBottomPanel::top("menu")
            .frame(frame(th.background))
            .show(ctx, |ui| menus(self, ui));
        egui::TopBottomPanel::top("tools")
            .frame(frame(th.background))
            .show(ctx, |ui| toolbar(ui, &mut self.st));
        egui::TopBottomPanel::bottom("status")
            .frame(frame(th.background))
            .show(ctx, |ui| status(ui, &self.st));
        egui::TopBottomPanel::bottom("transport")
            .frame(frame(th.panel))
            .show(ctx, |ui| transport(ui, &mut self.st));

        let mut style = Style::from_egui(ctx.style().as_ref());
        style.tab_bar.bg_fill = c(th.background);
        style.tab_bar.hline_color = c(th.border);
        style.tab.active.bg_fill = c(th.panel);
        style.tab.active.text_color = c(th.accent);
        style.tab.inactive.bg_fill = c(th.background);
        style.tab.inactive.text_color = c(th.text_dim);
        style.tab.focused.bg_fill = c(th.panel);
        style.tab.focused.text_color = c(th.accent);
        style.tab.hovered.text_color = c(th.text);
        style.tab.tab_body.bg_fill = c(th.panel);
        style.separator.color_idle = c(th.background);
        style.separator.color_hovered = c(th.accent);
        style.separator.color_dragged = c(th.accent_alt);
        style.separator.width = 3.0;

        let mut open_tab = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(c(th.background)))
            .show(ctx, |ui| {
                let mut viewer = Viewer {
                    st: &mut self.st,
                    caches: &mut self.viewer,
                    open_tab: &mut open_tab,
                };
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_inside(ui, &mut viewer);
            });
        if let Some(t) = open_tab {
            self.show_tab(t);
        }
        generate_window(ctx, &mut self.st);
        about_window(ctx, &mut self.st);
        if self.st.is_playing()
            || self.st.is_recording()
            || !self.st.jobs.is_empty()
            || self.st.separation.job.is_some()
            || self.st.meter.0 > 0.001
        {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

/// Shared helpers for panels.
pub fn meter_db(x: f32) -> f32 {
    gain_to_db(x).max(-90.0)
}
