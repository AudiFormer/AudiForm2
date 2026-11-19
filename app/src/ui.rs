// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

//! The window: menu bar, tool strip, the dockable panel area, transport
//! and status bar. Every panel lives in the dock and can be dragged to a
//! new place, tabbed with others, split, or floated in its own window.

use egui::{RichText, Rounding};
use egui_dock::{DockArea, DockState, Style, TabViewer};
use serde::{Deserialize, Serialize};

use crate::keymap::{Cmd, Keymap};
use crate::state::{data_dir, Mode, State, Tool};
use crate::theme::c;
use af27_core::time::{format_time, gain_to_db};
use af27_dsp::Effect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
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
    Inspector,
    Space,
    PianoRoll,
    Visualizer,
}

impl Tab {
    pub const ALL: [Tab; 16] = [
        Tab::Visualizer,
        Tab::Inspector,
        Tab::Space,
        Tab::PianoRoll,
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
    /// The title for a crowded tab bar.
    pub fn short_title(self) -> &'static str {
        match self {
            Tab::Statistics => "Statistics",
            Tab::Phase => "Phase",
            Tab::PianoRoll => "Piano",
            Tab::AiSeparation => "Stems",
            Tab::Visualizer => "Visuals",
            Tab::Inspector => "Track",
            t => t.title(),
        }
    }

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
            Tab::Phase => "Phase Wheel",
            Tab::Statistics => "Level Statistics",
            Tab::Spectrum => "Spectrum",
            Tab::Selection => "Range",
            Tab::Inspector => "Inspector",
            Tab::Space => "3D Space",
            Tab::PianoRoll => "Piano Roll",
            Tab::Visualizer => "Visualizer",
        }
    }
}

/// A tab's title: spaced capitals, or tighter and smaller when its panel
/// is crowded.
fn tab_text(t: &str, compact: bool) -> egui::WidgetText {
    let (size, spacing) = if compact { (10.5, 0.3) } else { (11.5, 1.2) };
    RichText::new(t.to_uppercase())
        .size(size)
        .extra_letter_spacing(spacing)
        .into()
}

/// About how wide a full-size tab is: its spaced title, padding and close
/// button.
fn full_tab_width(ctx: &egui::Context, title: &str) -> f32 {
    let up = title.to_uppercase();
    let w = ctx.fonts(|f| {
        f.layout_no_wrap(
            up.clone(),
            egui::FontId::proportional(11.5),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    w + up.chars().count() as f32 * 1.2 + 44.0
}

pub struct App {
    pub st: State,
    dock: DockState<Tab>,
    pub viewer: Caches,
    /// The docked cards as drawn last frame: their glass goes under them.
    cards: Vec<egui::Rect>,
    crowded: std::collections::HashSet<Tab>,
    showing: std::collections::HashSet<Tab>,
    boot_skipped: bool,
    /// Frame pacing for Auto visual effects.
    last_frame: f64,
    pace: f32,
    paced_frames: u32,
    frame_cpu_ms: f32,
    /// The keyboard shortcuts, and the windows of Edit > Keyboard
    /// Shortcuts and Edit > Preferences.
    pub keymap: Keymap,
    shortcuts: crate::settings::ShortcutsUi,
    settings: crate::settings::SettingsUi,
}

/// What a cached spectrogram was drawn from.
/// (document, its version, view start, view length, columns, rows, look).
pub type SpectroKey = (u64, u64, i64, i64, usize, usize, String);

/// Per-panel drawing caches (textures and the like) kept between frames.
#[derive(Default)]
pub struct Caches {
    pub spectro: Option<(egui::TextureHandle, SpectroKey)>,
    pub drag: crate::multitrack::Drag,
    pub wave_drag: Option<crate::waveform::WaveDrag>,
    pub marker_edit: Option<usize>,
    pub wheel: crate::phasewheel::WheelCache,
    pub space_drag: Option<usize>,
    pub piano: crate::pianoroll::PianoState,
}

/// Each workspace keeps its own arrangement, as the user left it.
fn layout_path(w: crate::workspace::Workspace) -> std::path::PathBuf {
    data_dir().join(format!("layout3-{}-v{}.json", w.key(), w.version()))
}

fn load_layout(w: crate::workspace::Workspace) -> DockState<Tab> {
    std::fs::read_to_string(layout_path(w))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| w.build())
}

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<std::path::PathBuf>) -> App {
        let mut st = State::new();
        crate::skin::install_fallback_fonts(&cc.egui_ctx);
        crate::hwcursor::install(cc);
        lay_colors(&mut st);
        st.theme.apply(&cc.egui_ctx);
        st.theme_changed = false;
        st.open_paths(files);
        let dock = load_layout(st.workspace);
        App {
            st,
            dock,
            viewer: Caches::default(),
            cards: Vec::new(),
            crowded: Default::default(),
            showing: Default::default(),
            boot_skipped: false,
            last_frame: 0.0,
            pace: 1.0 / 30.0,
            paced_frames: 0,
            frame_cpu_ms: 0.0,
            keymap: Keymap::load(),
            shortcuts: Default::default(),
            settings: Default::default(),
        }
    }

    fn save_layout(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string(&self.dock) {
            let _ = std::fs::write(layout_path(self.st.workspace), s);
        }
    }

    /// Keeps the current workspace's arrangement and shows another's.
    fn switch_workspace(&mut self, w: crate::workspace::Workspace) {
        if w == self.st.workspace {
            return;
        }
        self.save_layout();
        self.st.workspace = w;
        self.dock = load_layout(w);
        let _ = std::fs::write(data_dir().join("workspace.txt"), w.key());
    }

    fn show_tab(&mut self, tab: Tab) {
        if let Some(found) = self.dock.find_tab(&tab) {
            self.dock.set_active_tab(found);
        } else {
            self.dock.add_window(vec![tab]);
        }
    }
}

/// Lays the user's colour overrides (Preferences > Appearance) over the
/// theme as made with the user's hues.
fn lay_colors(st: &mut State) {
    let fresh = match &st.hues {
        Some(h) => st.theme_base.with_hues(h),
        None => st.theme_base.clone(),
    };
    let o = st.prefs.colors;
    o.apply(&mut st.theme, &fresh);
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_layout();
    }
}

struct Viewer<'a> {
    st: &'a mut State,
    caches: &'a mut Caches,
    open_tab: &'a mut Option<Tab>,
    tab_rects: &'a mut Vec<(Tab, egui::Rect)>,
    /// Tabs in panels too narrow for their full titles, and the tabs
    /// on show in each panel (as of the last frame).
    crowded: &'a std::collections::HashSet<Tab>,
    showing: &'a std::collections::HashSet<Tab>,
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
            return tab_text(&format!("{}: {name}", t), self.crowded.contains(tab));
        }
        if self.crowded.contains(tab) {
            return tab_text(tab.short_title(), true);
        }
        tab_text(t, false)
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        self.tab_rects.push((*tab, response.rect));
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        self.ui_inner(ui, tab);
    }

    fn closeable(&mut self, tab: &mut Tab) -> bool {
        // In a crowded panel only the tab on show keeps its close button.
        !self.crowded.contains(tab) || self.showing.contains(tab)
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Editor
            | Tab::Levels
            | Tab::Phase
            | Tab::Spectrum
            | Tab::PianoRoll
            | Tab::Visualizer
            | Tab::Space => [false, false],
            _ => [false, true],
        }
    }
}

impl Viewer<'_> {
    fn ui_inner(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Editor => {
                // The time display and transport sit along the bottom of
                // the Editor panel, as in the leading editor.
                egui::TopBottomPanel::bottom("editor-transport")
                    .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(4.0, 4.0)))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| transport(ui, self.st));
                match self.st.mode {
                    Mode::Waveform => crate::waveform::editor(ui, self.st, self.caches),
                    Mode::Multitrack => crate::multitrack::editor(ui, self.st, self.caches),
                }
            }
            Tab::Mixer => crate::multitrack::mixer(ui, self.st),
            Tab::Files => crate::panels::files(ui, self.st, self.open_tab),
            Tab::Markers => crate::panels::markers(ui, self.st, self.caches),
            Tab::Effects => crate::panels::effects(ui, self.st),
            Tab::AiSeparation => crate::separation::show(ui, self.st),
            Tab::History => crate::panels::history(ui, self.st),
            Tab::Levels => crate::panels::levels(ui, self.st),
            Tab::Phase => crate::phasewheel::panel(ui, self.st, &mut self.caches.wheel),
            Tab::Statistics => crate::panels::statistics(ui, self.st),
            Tab::Spectrum => crate::panels::spectrum(ui, self.st),
            Tab::Selection => crate::panels::range(ui, self.st),
            Tab::Inspector => crate::inspector::panel(ui, self.st),
            Tab::Space => crate::space::panel(ui, self.st, &mut self.caches.space_drag),
            Tab::PianoRoll => crate::pianoroll::panel(ui, self.st, &mut self.caches.piano),
            Tab::Visualizer => crate::visualizer::show(ui, self.st),
        }
    }
}

impl App {
    /// Runs the commands whose keys were pressed this frame.
    fn keys(&mut self, ctx: &egui::Context) {
        // A key being set in Keyboard Shortcuts is not a command.
        if self.shortcuts.capture.is_some() {
            return;
        }
        let typing = ctx.wants_keyboard_input();
        for cmd in self.keymap.pressed(ctx, typing) {
            self.run_cmd(cmd);
        }
        // Held Left / Right scrub at 4x (not while typing).
        if !typing {
            let (dir, dt) = ctx.input(|i| {
                let plain = !(i.modifiers.ctrl || i.modifiers.alt || i.modifiers.command);
                let l = plain && i.key_down(egui::Key::ArrowLeft);
                let r = plain && i.key_down(egui::Key::ArrowRight);
                (r as i32 - l as i32, i.stable_dt.min(0.1) as f64)
            });
            if dir != 0 {
                self.st.scrub_arrows(dir as f64, dt);
                ctx.request_repaint();
            }
        }
    }

    /// Does what a command does, from a key or a menu.
    fn run_cmd(&mut self, cmd: Cmd) {
        let st = &mut self.st;
        match cmd {
            Cmd::NewFile => st.new_file(48_000, 2),
            Cmd::Open => open_dialog(st),
            Cmd::Save => save(st),
            Cmd::Undo => st.undo(),
            Cmd::Redo => st.redo(),
            // Over the Piano Roll these act on its notes.
            Cmd::Cut | Cmd::Copy | Cmd::Paste | Cmd::SelectAll | Cmd::Delete
                if st.piano_hover && st.piano_clip.is_some() =>
            {
                use crate::pianoroll::PianoCmd;
                st.piano_cmd = Some(match cmd {
                    Cmd::Cut => PianoCmd::Cut,
                    Cmd::Copy => PianoCmd::Copy,
                    Cmd::Paste => PianoCmd::Paste,
                    Cmd::SelectAll => PianoCmd::SelectAll,
                    _ => PianoCmd::Delete,
                });
            }
            Cmd::Cut => st.edit_cut(),
            Cmd::Copy => st.edit_copy(),
            Cmd::Paste => st.edit_paste(),
            Cmd::Crop => st.edit_crop(),
            Cmd::SelectAll => st.select_all(),
            Cmd::Delete => st.edit_delete("Delete"),
            Cmd::AddMarker => st.add_marker(),
            Cmd::ClearSelection => match st.mode {
                Mode::Waveform => {
                    if let Some(d) = st.doc_mut() {
                        d.sel = None;
                    }
                }
                Mode::Multitrack => {
                    st.session_sel = None;
                    st.selected_clip = None;
                }
            },
            Cmd::KeyboardShortcuts => self.shortcuts.open = true,
            Cmd::Preferences => self.settings.open(),
            Cmd::SplitAtPlayhead => st.split_at_playhead(),
            Cmd::Invert => apply_now(st, Effect::Invert),
            Cmd::Reverse => apply_now(st, Effect::Reverse),
            Cmd::Silence => apply_now(st, Effect::Silence),
            Cmd::MatchLoudness => {
                // It has a target to set, so it opens in the Effects panel.
                st.effect = Effect::MatchLoudness;
                self.show_tab(Tab::Effects);
            }
            Cmd::AutoHeal => st.toast(AUTO_HEAL_NOTE),
            Cmd::PlayStop => st.toggle_play(),
            Cmd::Pause => st.toggle_pause(),
            Cmd::ReturnOnStop => {
                let on = !st.return_on_stop;
                st.set_return_on_stop(on);
            }
            Cmd::Record => {
                if st.is_recording() {
                    st.stop_recording();
                } else {
                    st.start_recording();
                }
            }
            Cmd::Loop => {
                let m = st.play_mode().toggle_loop();
                st.set_play_mode(m);
            }
            Cmd::ToStart => st.jump_to(0.0),
            Cmd::ToEnd => {
                let l = st.program_len();
                st.jump_to(l);
            }
            Cmd::WaveformView => st.mode = Mode::Waveform,
            Cmd::MultitrackView => st.mode = Mode::Multitrack,
            Cmd::ZoomIn => zoom(st, 0.5),
            Cmd::ZoomOut => zoom(st, 2.0),
            Cmd::ZoomFull => zoom_full(st),
            Cmd::MoveTool => st.tool = Tool::Move,
            Cmd::RazorTool => st.tool = Tool::Razor,
            Cmd::TimeSelectTool => st.tool = Tool::TimeSelect,
            Cmd::SaveAs => crate::export::open_save_as(st),
            Cmd::ExportMixdown => crate::export::open_mixdown(st),
            Cmd::ExportStems => crate::export::open_stems(st),
            Cmd::BounceTrack => crate::export::bounce_track(st),
            Cmd::RepeatEffect => match st.last_effect.clone() {
                Some((e, vals)) => {
                    st.effect_values.insert(e, vals);
                    apply_now(st, e);
                }
                None => st.toast("No effect to repeat yet"),
            },
            Cmd::RepeatEffectAdjust => match st.last_effect.clone() {
                Some((e, vals)) => {
                    st.effect_values.insert(e, vals);
                    st.effect = e;
                    self.show_tab(Tab::Effects);
                }
                None => st.toast("No effect to repeat yet"),
            },
        }
    }
}

const AUTO_HEAL_NOTE: &str = "Auto Heal is coming: there is no healing effect yet";

/// Runs a no-settings effect on the selection (or the whole file) at once.
fn apply_now(st: &mut State, e: Effect) {
    if st.doc().is_some() {
        st.mode = Mode::Waveform;
    }
    st.apply_effect(e);
}

/// Zoom In / Zoom Out: the playhead keeps its place (the middle of the
/// view when the playhead is out of view).
pub fn zoom(st: &mut State, f: f64) {
    let pos = st.position();
    let min = st.min_view_len();
    st.hand_moved_view();
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                let dur = d.duration();
                let anchor = crate::zoom::button_anchor(&d.view, pos);
                d.view.zoom_min(f, anchor, dur, min);
            }
        }
        Mode::Multitrack => {
            let dur = st.session.duration().max(30.0);
            let anchor = crate::zoom::button_anchor(&st.session_view, pos);
            st.session_view.zoom_min(f, anchor, dur, min);
        }
    }
}

/// Zoom to Selection: the selection fills the view.
fn zoom_selection(st: &mut State) {
    let Some((a, b)) = st.selection() else { return };
    let min = st.min_view_len();
    st.hand_moved_view();
    let fit = |v: &mut crate::state::View, total: f64| {
        let len = (b - a).max(min);
        *v = crate::state::View {
            start: (a + b) / 2.0 - len / 2.0,
            len,
        };
        v.clamp_to(total);
    };
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                let dur = d.duration();
                fit(&mut d.view, dur);
            }
        }
        Mode::Multitrack => {
            let dur = st.session.duration().max(30.0);
            fit(&mut st.session_view, dur);
        }
    }
}

fn zoom_full(st: &mut State) {
    st.hand_moved_view();
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
        Mode::Waveform => crate::export::save_in_place(st),
        Mode::Multitrack => {
            st.save_front(false);
        }
    }
}

fn save_session_as(st: &mut State) {
    st.save_front(true);
}

/// Multitrack > Insert Files as Tracks.
fn insert_files_as_tracks(st: &mut State) {
    if let Some(paths) = rfd::FileDialog::new()
        .add_filter("Audio", AUDIO_FILTER)
        .pick_files()
    {
        st.import_paths(paths, crate::sessions::Layout::AsTracks);
    }
}

fn open_props(st: &mut State) {
    st.sessions.props = Some(crate::sessions::PropsDraft {
        name: st.session.name.clone(),
        rate: st.session.rate,
    });
}

/// A menu item for a command, its key shown at the right.
fn cmd_item(
    ui: &mut egui::Ui,
    km: &Keymap,
    cmd: Cmd,
    label: &str,
    enabled: bool,
    out: &mut Option<Cmd>,
) -> egui::Response {
    let r = ui.add_enabled(
        enabled,
        egui::Button::new(label).shortcut_text(km.text(cmd)),
    );
    if r.clicked() {
        *out = Some(cmd);
        ui.close_menu();
    }
    r
}

/// A submenu that cannot open yet, and why.
fn disabled_submenu(ui: &mut egui::Ui, label: &str, why: &str) {
    ui.add_enabled_ui(false, |ui| {
        ui.menu_button(label, |_| {})
            .response
            .on_disabled_hover_text(why);
    });
}

const PLUGIN_NOTE: &str = "Plug-in hosting is coming";

fn menus(app: &mut App, ui: &mut egui::Ui) {
    let st = &mut app.st;
    let km = &app.keymap;
    let mut open_tab = None;
    let mut cmd: Option<Cmd> = None;
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            cmd_item(ui, km, Cmd::NewFile, "New Audio File", true, &mut cmd);
            if ui
                .button("New Multitrack Session")
                .on_hover_text("The session open now is set aside, not closed")
                .clicked()
            {
                st.new_session(None);
                ui.close_menu();
            }
            ui.menu_button("New Session from Template", |ui| {
                let templates = crate::sessions::list_templates();
                if templates.is_empty() {
                    ui.label("No templates yet: File > Save as Session Template.");
                }
                for t in templates {
                    let name = t
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if ui.button(name).clicked() {
                        st.new_session(Some(&t));
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Open Templates Folder").clicked() {
                    let dir = crate::sessions::templates_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    crate::sessions::reveal(&dir);
                    ui.close_menu();
                }
            });
            cmd_item(ui, km, Cmd::Open, "Open...", true, &mut cmd);
            ui.menu_button("Open Recent", |ui| {
                let recent = st.sessions.recent.clone();
                if recent.is_empty() {
                    ui.label("No sessions yet.");
                }
                let mut gone = None;
                for (i, p) in recent.iter().enumerate() {
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if ui
                        .button(format!("{}  {name}", i + 1))
                        .on_hover_text(p.display().to_string())
                        .clicked()
                    {
                        ui.close_menu();
                        if p.exists() {
                            st.open_paths(vec![p.clone()]);
                        } else {
                            st.error(format!("{} is no longer there.", p.display()));
                            gone = Some(i);
                        }
                    }
                }
                if let Some(i) = gone {
                    st.sessions.recent.remove(i);
                    crate::sessions::save_recent(&st.sessions.recent);
                }
                if !recent.is_empty() {
                    ui.separator();
                    if ui.button("Clear Recent").clicked() {
                        st.sessions.recent.clear();
                        crate::sessions::save_recent(&st.sessions.recent);
                        ui.close_menu();
                    }
                }
            });
            ui.menu_button("Import", |ui| {
                if ui
                    .button("Files as Tracks...")
                    .on_hover_text("Each file on its own new track at the start, named without a leading speaker number")
                    .clicked()
                {
                    ui.close_menu();
                    insert_files_as_tracks(st);
                }
                if ui.button("MIDI File...").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("MIDI files", &["mid", "midi", "smf"])
                        .pick_file()
                    {
                        crate::tempo::import_midi(st, p);
                    }
                }
            });
            ui.separator();
            cmd_item(ui, km, Cmd::Save, "Save", true, &mut cmd);
            let has_doc = st.doc().is_some();
            cmd_item(ui, km, Cmd::SaveAs, "Save Audio As...", has_doc, &mut cmd);
            if ui.button("Save Session As...").clicked() {
                ui.close_menu();
                save_session_as(st);
            }
            if ui
                .button("Save All")
                .on_hover_text("Every open session with unsaved changes")
                .clicked()
            {
                ui.close_menu();
                st.save_all();
            }
            if ui.button("Save as Session Template...").clicked() {
                ui.close_menu();
                st.save_template();
            }
            ui.separator();
            if ui.button("Close File").clicked() {
                if let Some(id) = st.current {
                    st.close_doc(id);
                }
                ui.close_menu();
            }
            if ui
                .add_enabled(!st.front_is_blank(), egui::Button::new("Close Session"))
                .clicked()
            {
                ui.close_menu();
                st.close_front();
            }
            if ui.button("Close All Sessions").clicked() {
                ui.close_menu();
                st.close_all_sessions();
            }
            ui.separator();
            if ui
                .button("Remove Unused Files")
                .on_hover_text("Close every file no clip in an open session plays")
                .clicked()
            {
                ui.close_menu();
                st.remove_unused_files();
            }
            if ui
                .add_enabled(
                    st.session_path.is_some(),
                    egui::Button::new("Collect Session Media"),
                )
                .on_hover_text("Copy every file the session uses into its folder")
                .on_disabled_hover_text("Save the session first")
                .clicked()
            {
                ui.close_menu();
                st.collect_media();
            }
            if ui.button("Session Properties...").clicked() {
                open_props(st);
                ui.close_menu();
            }
            let shown = match st.mode {
                Mode::Multitrack => st.session_path.clone(),
                Mode::Waveform => st.doc().and_then(|d| d.path.clone()),
            };
            if ui
                .add_enabled(shown.is_some(), egui::Button::new("Show in File Manager"))
                .clicked()
            {
                if let Some(p) = shown {
                    crate::sessions::reveal(&p);
                }
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button("Export", |ui| {
                cmd_item(
                    ui,
                    km,
                    Cmd::ExportMixdown,
                    "Multitrack Mixdown...",
                    true,
                    &mut cmd,
                );
                cmd_item(ui, km, Cmd::ExportStems, "Track Stems...", true, &mut cmd);
                if st.session.space.is_some()
                    && ui.button("3D Speaker Bed (multichannel)...").clicked()
                {
                    crate::export::open_bed(st);
                    ui.close_menu();
                }
            });
            ui.separator();
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
            let undo = match &ul {
                Some(l) => format!("Undo {l}"),
                None => "Undo".to_string(),
            };
            cmd_item(ui, km, Cmd::Undo, &undo, ul.is_some(), &mut cmd);
            cmd_item(ui, km, Cmd::Redo, "Redo", true, &mut cmd);
            ui.separator();
            for c in [
                Cmd::Cut,
                Cmd::Copy,
                Cmd::Paste,
                Cmd::Crop,
                Cmd::Delete,
                Cmd::SelectAll,
                Cmd::AddMarker,
            ] {
                cmd_item(ui, km, c, c.label(), true, &mut cmd);
            }
            ui.separator();
            cmd_item(
                ui,
                km,
                Cmd::KeyboardShortcuts,
                "Keyboard Shortcuts...",
                true,
                &mut cmd,
            );
            cmd_item(ui, km, Cmd::Preferences, "Preferences...", true, &mut cmd);
        });
        ui.menu_button("Multitrack", |ui| {
            if ui.button("Add Audio Track").clicked() {
                st.add_track_kind(af27_core::TrackKind::Audio);
                ui.close_menu();
            }
            if ui.button("Add Bus Track").clicked() {
                st.add_track_kind(af27_core::TrackKind::Bus);
                ui.close_menu();
            }
            if ui.button("Add MIDI Track").clicked() {
                let t = st.add_track_kind(af27_core::TrackKind::Midi);
                crate::pianoroll::new_clip(st, t, 0.0);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Tempo and Time Signature...").clicked() {
                st.tempo_ui.open = true;
                ui.close_menu();
            }
            let mut m = st.session.tempo.metronome;
            if ui.checkbox(&mut m, "Metronome").changed() {
                st.session.tempo.metronome = m;
                st.live_metronome();
            }
            playback_menu_items(ui, st, km, &mut cmd);
            ui.menu_button("Export", |ui| {
                if ui.button("MIDI File...").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new()
                        .set_file_name(format!("{}.mid", st.session.name))
                        .add_filter("MIDI file", &["mid"])
                        .save_file()
                    {
                        crate::tempo::export_midi(st, p);
                    }
                }
            });
            ui.separator();
            cmd_item(
                ui,
                km,
                Cmd::SplitAtPlayhead,
                "Split at Playhead",
                true,
                &mut cmd,
            );
            if ui.button("Delete Selected Clip").clicked() {
                st.delete_clip();
                ui.close_menu();
            }
            cmd_item(
                ui,
                km,
                Cmd::BounceTrack,
                "Bounce Selected Track to New Track",
                st.selected_track.is_some(),
                &mut cmd,
            );
            ui.separator();
            if ui
                .button("Insert Files as Tracks...")
                .on_hover_text("Each file on its own new track at the start, named without a leading speaker number")
                .clicked()
            {
                ui.close_menu();
                insert_files_as_tracks(st);
            }
            let picked = st.files_ui.selected.len();
            if ui
                .add_enabled(picked > 0, egui::Button::new("Insert Selected Files as Tracks"))
                .on_disabled_hover_text("Pick files in the Files panel first")
                .clicked()
            {
                let order: Vec<_> = st.docs.iter().map(|d| d.id).collect();
                let docs = st.files_ui.picked_in(&order);
                st.import_docs(docs, crate::sessions::Layout::AsTracks);
                ui.close_menu();
            }
            if ui.button("Session Properties...").clicked() {
                open_props(st);
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
            effects_menu(ui, st, km, &mut cmd, &mut open_tab);
        });
        ui.menu_button("View", |ui| {
            for (c, m) in [
                (Cmd::WaveformView, Mode::Waveform),
                (Cmd::MultitrackView, Mode::Multitrack),
            ] {
                let b = egui::Button::new(c.label())
                    .selected(st.mode == m)
                    .shortcut_text(km.text(c));
                if ui.add(b).clicked() {
                    cmd = Some(c);
                    ui.close_menu();
                }
            }
            ui.separator();
            if let Some(d) = st.doc_mut() {
                ui.checkbox(&mut d.spectral, "Spectral Display");
            }
            for c in [Cmd::ZoomIn, Cmd::ZoomOut, Cmd::ZoomFull] {
                if ui
                    .add(egui::Button::new(c.label()).shortcut_text(km.text(c)))
                    .clicked()
                {
                    cmd = Some(c);
                }
            }
            ui.menu_button("Time Display", |ui| time_display_items(ui, st));
            if ui
                .add_enabled(
                    st.selection().is_some(),
                    egui::Button::new("Zoom to Selection"),
                )
                .clicked()
            {
                zoom_selection(st);
            }
            let mut follow = st.prefs.follow_playhead;
            if ui.checkbox(&mut follow, "Follow Playhead").changed() {
                st.prefs.follow_playhead = follow;
                st.follow.set_on(follow);
                st.prefs.save();
            }
            ui.separator();
            ui.menu_button("Visual Effects", |ui| {
                for (m, label, tip) in [
                    (0u8, "Auto", "Full effects, cut back on their own if this device draws slowly"),
                    (1, "Full", "Every effect, always"),
                    (2, "Lite", "No moving backdrop, glow or scan lines: lightest on slow or battery-powered devices"),
                ] {
                    if ui.radio(st.visuals_mode == m, label).on_hover_text(tip).clicked() {
                        st.visuals_mode = m;
                        st.visuals_lite = false;
                        let _ = std::fs::create_dir_all(data_dir());
                        let _ = std::fs::write(data_dir().join("visuals.txt"), m.to_string());
                        ui.close_menu();
                    }
                }
                if st.visuals_mode == 0 && st.visuals_lite {
                    ui.label(RichText::new("Auto chose Lite for this device").small().color(c(st.theme.text_dim)));
                }
            });
            crate::pointers::menu_toggle(ui);
            ui.menu_button("Theme Colors", |ui| {
                ui.label(
                    RichText::new(format!("Your own hues for {}", st.theme_base.name))
                        .small()
                        .color(c(st.theme.text_dim)),
                );
                let mut h = st.hues.unwrap_or_else(|| st.theme_base.hues());
                let mut changed = false;
                for (label, value, sat, val) in [
                    ("Main light", &mut h.primary, 1.0, 1.0),
                    ("Second light", &mut h.secondary, 0.82, 1.0),
                    ("Base tint", &mut h.surface, 0.6, 0.35),
                ] {
                    ui.horizontal(|ui| {
                        let sw = crate::theme::from_hsv(*value, sat, val);
                        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 2.0, c(sw));
                        changed |= ui
                            .add(egui::Slider::new(value, 0.0..=359.0).suffix("°").text(label))
                            .changed();
                    });
                }
                if changed {
                    st.set_hues(Some(h));
                }
                ui.horizontal(|ui| {
                    for (name, p, s, u) in [
                        ("Ice", 187.0, 325.0, 228.0),
                        ("Toxic", 105.0, 285.0, 150.0),
                        ("Ember", 22.0, 350.0, 15.0),
                        ("Ultraviolet", 275.0, 190.0, 260.0),
                        ("Gold", 45.0, 200.0, 220.0),
                    ] {
                        if ui.small_button(name).clicked() {
                            st.set_hues(Some(crate::theme::Hues { primary: p, secondary: s, surface: u }));
                        }
                    }
                });
                if ui.button("Back to the theme's own colors").clicked() {
                    st.set_hues(None);
                    ui.close_menu();
                }
            });
            ui.menu_button("Theme", |ui| {
                let names: Vec<String> = st.themes.iter().map(|t| t.name.clone()).collect();
                let classic = st.themes.iter().position(|t| t.author == "Audiform");
                for (i, n) in names.iter().enumerate() {
                    let on = st.theme_base.name == *n;
                    if st.themes[i].hidden && !on {
                        continue;
                    }
                    if Some(i) == classic {
                        ui.separator();
                        ui.label(RichText::new("From Audiform").small().color(c(st.theme.text_dim)));
                    }
                    let label = if st.themes[i].hidden { "Secret Theme" } else { n.as_str() };
                    if ui.radio(on, label).clicked() {
                        let t = st.themes[i].clone();
                        st.set_theme(t);
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
                        .add_filter("AudiForm 2 theme", &["af27theme"])
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
                ui.separator();
                ui.label(RichText::new("HUD").small().color(c(st.theme.text_dim)));
                ui.horizontal(|ui| {
                    for g in ["perspective", "hex", "dots", "lines", "none"] {
                        if ui.selectable_label(st.theme.grid_style == g, g).clicked() {
                            st.theme.grid_style = g.into();
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut st.theme.grid_strength, 0.0..=2.0).text("Grid"));
                ui.add(egui::Slider::new(&mut st.theme.glass, 0.3..=1.0).text("Glass"));
                ui.add(egui::Slider::new(&mut st.theme.chamfer, 0.0..=20.0).text("Corner cut"));
                ui.add(egui::Slider::new(&mut st.theme.scanlines, 0.0..=0.6).text("Scan lines"));
                ui.add(egui::Slider::new(&mut st.theme.motion, 0.0..=2.0).text("Motion"));
                ui.add(egui::Slider::new(&mut st.theme.reactive, 0.0..=1.5).text("Reacts to music"));
                ui.checkbox(&mut st.theme.brackets, "Corner brackets");
                ui.checkbox(&mut st.theme.intro, "Boot sequence");
                if !st.theme.columns.is_empty() {
                    ui.checkbox(&mut st.theme.realistic, "Realistic marble and tile");
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
                app.dock = st.workspace.build();
                ui.close_menu();
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("About AudiForm 2").clicked() {
                st.show_about = true;
                ui.close_menu();
            }
        });
    });
    if let Some(t) = open_tab {
        app.show_tab(t);
    }
    if let Some(c) = cmd {
        app.run_cmd(c);
    }
}

/// The Effects menu, laid out as in the leading editor.
fn effects_menu(
    ui: &mut egui::Ui,
    st: &mut State,
    km: &Keymap,
    cmd: &mut Option<Cmd>,
    open_tab: &mut Option<Tab>,
) {
    let last = st.last_effect.as_ref().map(|(e, _)| e.name());
    let (repeat, adjust) = match last {
        Some(n) => (format!("Repeat {n}"), format!("Repeat {n} with Changes...")),
        None => (
            "Repeat Last Effect".to_string(),
            "Repeat Last Effect with Changes...".to_string(),
        ),
    };
    cmd_item(ui, km, Cmd::RepeatEffect, &repeat, last.is_some(), cmd);
    cmd_item(
        ui,
        km,
        Cmd::RepeatEffectAdjust,
        &adjust,
        last.is_some(),
        cmd,
    );
    ui.separator();
    let multi = st.mode == Mode::Multitrack;
    // Clips have no rack of their own yet: a clip's track's rack opens.
    let clip_track = st
        .selected_clip
        .and_then(|id| st.session.find_clip(id))
        .map(|(ti, _)| st.session.tracks[ti].id);
    if ui
        .add_enabled(
            multi && clip_track.is_some(),
            egui::Button::new("Edit Clip Effects in Effects Rack"),
        )
        .on_hover_text(
            "Clips play through their track's rack: this opens the rack of the clip's track",
        )
        .on_disabled_hover_text("Select a clip in the Multitrack editor")
        .clicked()
    {
        st.selected_track = clip_track;
        *open_tab = Some(Tab::Inspector);
        ui.close_menu();
    }
    if ui
        .add_enabled(
            multi && st.selected_track.is_some(),
            egui::Button::new("Edit Track Effects in Effects Rack"),
        )
        .on_disabled_hover_text("Select a track in the Multitrack editor")
        .clicked()
    {
        *open_tab = Some(Tab::Inspector);
        ui.close_menu();
    }
    ui.separator();
    let has_doc = st.doc().is_some();
    let no_doc = "Open a file in the Waveform editor first";
    for c in [Cmd::Invert, Cmd::Reverse, Cmd::Silence] {
        cmd_item(ui, km, c, c.label(), has_doc, cmd).on_disabled_hover_text(no_doc);
    }
    ui.menu_button("Generate", |ui| {
        if ui.button("Tone or Noise...").clicked() {
            st.generate.open = true;
            ui.close_menu();
        }
        if ui.button("Silence (5 s)").clicked() {
            let (rate, ch) = st
                .doc()
                .map_or((48_000, 2), |d| (d.buf.rate, d.buf.num_channels()));
            st.insert_buffer("Insert silence", af27_dsp::generate::silence(rate, ch, 5.0));
            ui.close_menu();
        }
    });
    ui.menu_button("Spectrum", |ui| {
        crate::spectral_select::effects_submenu(ui, st)
    });
    ui.separator();
    cmd_item(ui, km, Cmd::MatchLoudness, "Match Loudness", true, cmd);
    ui.add_enabled(
        false,
        egui::Button::new("Auto Heal Selection").shortcut_text(km.text(Cmd::AutoHeal)),
    )
    .on_disabled_hover_text(AUTO_HEAL_NOTE);
    ui.separator();
    let show = st.prefs.show_effects_on_pick;
    for (cat, items) in crate::fx_menu::categories() {
        if items.is_empty() {
            disabled_submenu(ui, cat, "Nothing here yet");
            continue;
        }
        ui.menu_button(cat, |ui| {
            for item in items {
                if ui.button(item.name()).clicked() {
                    match item {
                        crate::fx_menu::Item::Builtin(e) => {
                            st.effect = e;
                            if show {
                                *open_tab = Some(Tab::Effects);
                            }
                        }
                        crate::fx_menu::Item::Fx(id, _) => {
                            st.fx_pick = Some(id);
                            if show {
                                *open_tab = Some(Tab::Effects);
                            }
                        }
                        crate::fx_menu::Item::Stems => *open_tab = Some(Tab::AiSeparation),
                    }
                    ui.close_menu();
                }
            }
        });
    }
    ui.separator();
    disabled_submenu(ui, "VST", PLUGIN_NOTE);
    disabled_submenu(ui, "VST 3", PLUGIN_NOTE);
    ui.add_enabled(false, egui::Button::new("Audio Plug-In Manager..."))
        .on_disabled_hover_text(PLUGIN_NOTE);
}

/// The spectral selection tools (Waveform editor only) and the brush size.
fn spectral_tools(ui: &mut egui::Ui, st: &mut State) {
    ui.separator();
    let wave = st.mode == Mode::Waveform;
    for (tool, label, tip) in [
        (
            Tool::BoxSelect,
            "Box",
            "Box Select: drag a rectangle in the spectrum (Shift adds)",
        ),
        (
            Tool::LassoSelect,
            "Lasso",
            "Lasso Select: draw around a sound in the spectrum (Shift adds)",
        ),
        (
            Tool::BrushSelect,
            "Brush",
            "Brush Select: paint over a sound in the spectrum (Shift adds)",
        ),
        (
            Tool::SpotRepair,
            "Spot",
            "Spot Repair: paint over a sound; it is repaired when you let go",
        ),
    ] {
        if ui
            .add_enabled(
                wave,
                egui::SelectableLabel::new(st.tool == tool, RichText::new(label).size(12.5)),
            )
            .on_hover_text(tip)
            .on_disabled_hover_text("Spectral tools work in the Waveform editor")
            .clicked()
        {
            st.choose_tool(tool);
        }
    }
    if wave && matches!(st.tool, Tool::BrushSelect | Tool::SpotRepair) {
        ui.add(
            egui::Slider::new(&mut st.brush_px, 4.0..=120.0)
                .integer()
                .suffix(" px"),
        )
        .on_hover_text("Brush size");
    }
}

fn toolbar(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let fx = st.fx.clone();
        let th = st.theme.clone();
        let cur = if st.mode == Mode::Waveform { 0 } else { 1 };
        match crate::hud::segmented(ui, "af27-mode", &["Waveform", "Multitrack"], cur, &th, &fx) {
            Some(0) => st.mode = Mode::Waveform,
            Some(1) => st.mode = Mode::Multitrack,
            _ => {}
        }
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
        spectral_tools(ui, st);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            // A theme with its own title shows it in the menu bar instead.
            if th.title_text.is_empty() && th.title_picture.is_empty() {
                crate::hud::wordmark(ui, &th, &fx);
            }
            ui.add_space(18.0);
            // The workspace, chosen from one drop-down in the top right.
            let mut pick = st.workspace;
            let mut reset = false;
            egui::ComboBox::from_id_salt("af2-workspace")
                .selected_text(
                    RichText::new(st.workspace.name().to_uppercase())
                        .size(11.0)
                        .extra_letter_spacing(1.2)
                        .color(c(th.accent)),
                )
                .width(150.0)
                .show_ui(ui, |ui| {
                    for w in crate::workspace::Workspace::ALL {
                        ui.selectable_value(&mut pick, w, w.name())
                            .on_hover_text(w.about());
                    }
                    ui.separator();
                    if ui.button("Reset to Saved Layout").clicked() {
                        reset = true;
                    }
                })
                .response
                .on_hover_text("Workspace");
            ui.label(
                RichText::new("WORKSPACE")
                    .size(9.5)
                    .extra_letter_spacing(1.5)
                    .color(c(th.text_dim)),
            );
            if pick != st.workspace {
                st.want_workspace = Some(pick);
            }
            if reset {
                st.reset_workspace = true;
            }
        });
    });
}

/// Reads a typed time: `h:mm:ss.mmm`, `m:ss.mmm` or plain seconds.
#[cfg(test)]
pub fn parse_time(text: &str) -> Option<f64> {
    crate::transport::parse_plain(text)
}

/// The hidden theme a phrase opens, if it is one. As in Audiform, only
/// letters are ever hashed, so ordinary times never are.
pub fn theme_for_phrase<'a>(
    themes: &'a [crate::theme::Theme],
    text: &str,
) -> Option<&'a crate::theme::Theme> {
    use sha2::{Digest, Sha256};
    let t = text.trim().to_lowercase();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let hash: String = Sha256::digest(t.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    themes.iter().find(|th| th.hidden && th.unlock == hash)
}

/// What Enter in the time display does: open a hidden theme for its
/// phrase, or move the playhead to the time typed.
fn enter_time(st: &mut State, text: &str) {
    if let Some(t) = theme_for_phrase(&st.themes, text).cloned() {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(data_dir().join("theme.txt"), &t.name);
        st.set_theme(t);
        st.toast("Secret Theme found");
    } else if let Some(t) = st.time_ctx().parse(text) {
        st.jump_to(t);
    }
}

/// The markers of what the editor shows, in time order.
fn marker_times(st: &State) -> Vec<f64> {
    let mut v: Vec<f64> = match st.mode {
        Mode::Waveform => st
            .doc()
            .map(|d| d.markers.iter().map(|m| m.time).collect())
            .unwrap_or_default(),
        Mode::Multitrack => st.session.markers.iter().map(|m| m.time).collect(),
    };
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

/// The Time Display choices (View menu, the ruler's and the clock's
/// right-click menus).
pub fn time_display_items(ui: &mut egui::Ui, st: &mut State) {
    for f in crate::transport::TimeFormat::ALL {
        if ui.radio(st.prefs.time_format == f, f.label()).clicked() {
            st.prefs.time_format = f;
            st.prefs.save();
            ui.close_menu();
        }
    }
}

/// Pause, the Loop / Skip mode and Return to Start on Stop (Multitrack menu).
fn playback_menu_items(ui: &mut egui::Ui, st: &mut State, km: &Keymap, cmd: &mut Option<Cmd>) {
    ui.separator();
    let pause = egui::Button::new("Pause")
        .selected(st.paused)
        .shortcut_text(km.text(Cmd::Pause));
    if ui
        .add_enabled(st.transport_playing() || st.paused, pause)
        .clicked()
    {
        *cmd = Some(Cmd::Pause);
        ui.close_menu();
    }
    play_mode_items(ui, st, km);
    let ret = egui::Button::new("Return to Start on Stop")
        .selected(st.return_on_stop)
        .shortcut_text(km.text(Cmd::ReturnOnStop));
    if ui.add(ret).clicked() {
        *cmd = Some(Cmd::ReturnOnStop);
        ui.close_menu();
    }
}

/// Normal / Loop Playback / Skip Selection, as radio items.
fn play_mode_items(ui: &mut egui::Ui, st: &mut State, km: &Keymap) {
    use crate::transport::PlayMode;
    for m in PlayMode::ALL {
        let key = if m == PlayMode::Loop {
            km.text(Cmd::Loop)
        } else {
            String::new()
        };
        let b = egui::Button::new(m.label())
            .selected(st.play_mode() == m)
            .shortcut_text(key);
        if ui.add(b).clicked() {
            st.set_play_mode(m);
            ui.close_menu();
        }
    }
}

fn transport(ui: &mut egui::Ui, st: &mut State) {
    let th = st.theme.clone();
    ui.horizontal(|ui| {
        // Big timecode, in the chosen time format.
        let fx = st.fx.clone();
        let tc = st.time_ctx();
        let t = tc.format(st.position());
        let caption = if st.is_recording() {
            "REC // TIME"
        } else if st.transport_playing() {
            "PLAY // TIME"
        } else if st.paused {
            "PAUSED // TIME"
        } else {
            "TIME"
        };
        // Click the time to type one; Enter moves the playhead there.
        if let Some(mut text) = st.time_edit.take() {
            let r = ui.add_sized(
                crate::hud::READOUT,
                egui::TextEdit::singleline(&mut text)
                    .font(egui::FontId::monospace(15.0))
                    .text_color(c(th.accent))
                    .vertical_align(egui::Align::Center),
            );
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if r.lost_focus() || escape {
                if enter {
                    enter_time(st, &text);
                }
            } else {
                st.time_edit = Some(text);
            }
        } else {
            let r = crate::hud::readout(ui, &t, caption, &th, &fx, st.meter)
                .on_hover_text("Click to type a time; right-click for the time display");
            if r.clicked() {
                st.time_edit = Some(tc.format(st.cursor()));
            }
            r.context_menu(|ui| time_display_items(ui, st));
        }
        // The time display format: click for the next one.
        let f = st.prefs.time_format;
        if ui
            .add_sized(
                egui::vec2(34.0, 26.0),
                egui::Button::new(RichText::new(f.short()).monospace().small()),
            )
            .on_hover_text(format!(
                "Time Display: {} (click for the next format)",
                f.label()
            ))
            .clicked()
        {
            st.prefs.time_format = f.next();
            st.prefs.save();
        }
        ui.add_space(6.0);
        // Bars and beats, the tempo and the metronome, all on the one row
        // and as tall as the buttons.
        ui.add_sized(
            egui::vec2(64.0, 26.0),
            egui::Label::new(
                egui::RichText::new(st.session.tempo.format(st.position()))
                    .monospace()
                    .color(c(th.text_dim)),
            ),
        );
        // The tempo and time signature of the stretch the playhead is in.
        let seg = st.session.tempo.seg_at_secs(st.position().max(0.0));
        let mut bpm = seg.bpm;
        let r = ui
            .add_sized(
                egui::vec2(78.0, 26.0),
                egui::DragValue::new(&mut bpm)
                    .range(af27_core::midi::BPM_MIN..=af27_core::midi::BPM_MAX)
                    .speed(0.5)
                    .suffix(" BPM")
                    .max_decimals(2),
            )
            .on_hover_text("Tempo where the playhead is: drag, or double-click to type");
        if r.drag_started() {
            st.gesture_before = Some(st.session.clone());
        }
        if r.changed() {
            let before = st.session.clone();
            st.session
                .tempo
                .set_values(seg.change, bpm, seg.beats_per_bar, seg.beat_unit);
            st.session_version += 1;
            st.live_metronome();
            if !r.dragged() && st.gesture_before.is_none() {
                st.commit_session("Tempo", before);
            }
        }
        if r.drag_stopped() || (r.lost_focus() && st.gesture_before.is_some()) {
            if let Some(b) = st.gesture_before.take() {
                if b != st.session {
                    st.commit_session("Tempo", b);
                }
            }
        }
        if ui
            .add_sized(
                egui::vec2(36.0, 26.0),
                egui::Button::new(seg.meter_text()).frame(false),
            )
            .on_hover_text(
                "Time signature where the playhead is (Multitrack > Tempo and Time Signature...)",
            )
            .clicked()
        {
            st.tempo_ui.open = true;
        }
        let m = st.session.tempo.metronome;
        if crate::hud::transport_button(
            ui,
            crate::hud::Icon::Metronome,
            "Metronome",
            m,
            th.accent,
            &th,
        )
        .clicked()
        {
            st.session.tempo.metronome = !m;
            st.live_metronome();
        }
        ui.add_space(14.0);
        // The transport, in the order and style long-time editors expect:
        // Stop, Play, Pause, previous, Rewind, Fast Forward, next, Record,
        // Loop.
        use crate::hud::{transport_button as tb, Icon};
        let playing = st.transport_playing();
        let rec = st.is_recording();
        if tb(ui, Icon::Stop, "Stop (Space)", false, th.text, &th).clicked() {
            if rec {
                st.stop_recording();
            } else {
                st.stop();
            }
        }
        if tb(ui, Icon::Play, "Play (Space)", playing, th.accent, &th).clicked() && !playing {
            st.play();
        }
        if tb(
            ui,
            Icon::Pause,
            "Pause: the playhead and start marker stay; Pause or Play continues",
            st.paused,
            th.accent,
            &th,
        )
        .clicked()
        {
            st.toggle_pause();
        }
        let marks = marker_times(st);
        if tb(
            ui,
            Icon::ToStart,
            "Move Playhead to Previous (marker, or the start)",
            false,
            th.text,
            &th,
        )
        .clicked()
        {
            let now = st.position();
            let t = marks
                .iter()
                .rev()
                .copied()
                .find(|&m| m < now - 1e-3)
                .unwrap_or(0.0);
            st.jump_to(t);
        }
        // Rewind and Fast Forward: 5 s a click, 1 s with Shift.
        let step = if ui.input(|i| i.modifiers.shift) {
            1.0
        } else {
            5.0
        };
        if tb(
            ui,
            Icon::Rewind,
            "Rewind 5 s (1 s with Shift)",
            false,
            th.text,
            &th,
        )
        .clicked()
        {
            let now = st.position();
            st.jump_to(now - step);
        }
        if tb(
            ui,
            Icon::FastForward,
            "Fast Forward 5 s (1 s with Shift)",
            false,
            th.text,
            &th,
        )
        .clicked()
        {
            let now = st.position();
            st.jump_to(now + step);
        }
        if tb(
            ui,
            Icon::ToEnd,
            "Move Playhead to Next (marker, or the end)",
            false,
            th.text,
            &th,
        )
        .clicked()
        {
            let now = st.position();
            let end = st.program_len();
            let t = marks
                .iter()
                .copied()
                .find(|&m| m > now + 1e-3)
                .unwrap_or(end);
            st.jump_to(t);
        }
        if tb(
            ui,
            Icon::Record,
            "Record (Shift+Space)",
            rec,
            th.accent_alt,
            &th,
        )
        .clicked()
        {
            if rec {
                st.stop_recording();
            } else {
                st.start_recording();
            }
        }
        // Loop / Skip: one mode, the time selection is the range.
        use crate::transport::PlayMode;
        let mode = st.play_mode();
        let col = if mode == PlayMode::Skip {
            th.accent_alt
        } else {
            th.accent
        };
        let tip = format!(
            "{} (click: Normal, Loop, Skip Selection; right-click to pick; Ctrl+L loops)",
            mode.label()
        );
        let r = tb(ui, Icon::Loop, &tip, mode != PlayMode::Normal, col, &th);
        if r.clicked() {
            st.set_play_mode(mode.next());
        }
        r.context_menu(|ui| {
            for m in PlayMode::ALL {
                if ui.radio(st.play_mode() == m, m.label()).clicked() {
                    st.set_play_mode(m);
                    ui.close_menu();
                }
            }
        });
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
        } else if let Some(p) = crate::spectral_select::progress(st) {
            ui.spinner();
            ui.label(format!("Spectral edit {:.0}%", p * 100.0));
        } else if !st.loading().is_empty() {
            ui.spinner();
            ui.label(format!("Opening {}", st.loading().join(", ")));
        } else if let Some(e) = &st.engine_error {
            ui.label(RichText::new(e).color(c(th.accent_alt)));
        } else {
            ui.label(RichText::new("Ready").color(c(th.text_dim)));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::skin::status_line(ui, th);
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
    egui::Window::new("About AudiForm 2").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(120.0, 120.0), egui::Sense::hover());
        crate::hud::logo_mark(ui.painter(), r, &st.theme, &st.fx, 1.0);
        ui.label(RichText::new("AUDIFORM 2").size(28.0).strong().extra_letter_spacing(6.0).color(c(st.theme.accent)));
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label("A fast, themeable audio editor written entirely in Rust.");
        ui.label("AudiForm 2 License 1.0 (see LICENSE). The DTS decoder (af27-dts) is LGPL-2.1-or-later, ported from FFmpeg.");
        ui.label("The sequel to Audiform, by the same project lead.");
    });
    st.show_about = open;
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::selfshot::tick(ctx, ctx.input(|i| i.time));
        let __t0 = std::time::Instant::now();
        struct TraceGuard(std::time::Instant, egui::Rect);
        impl Drop for TraceGuard {
            fn drop(&mut self) {
                if std::env::var_os("AF2_TRACE").is_some() {
                    eprintln!(
                        "frame {:?} {:.2} ms",
                        self.1,
                        self.0.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
        }
        let _trace = TraceGuard(__t0, ctx.screen_rect());
        if self.st.theme_changed {
            lay_colors(&mut self.st);
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
            // Files from the file manager: the Multitrack takes them if
            // they landed on its lanes (where the pointer and Alt are read
            // from the system: the window heard neither during the drag);
            // anywhere else they open.
            let ppp = ctx.pixels_per_point();
            let pos = crate::hwcursor::system_pointer()
                .map(|(x, y)| egui::pos2(x / ppp, y / ppp))
                .or(ctx.input(|i| i.pointer.hover_pos()));
            let alt = crate::hwcursor::alt_down() || ctx.input(|i| i.modifiers.alt);
            self.st.sessions.os_drop = Some(crate::sessions::OsDrop {
                paths: dropped,
                pos,
                alt,
            });
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.st.may_quit() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.st.tick();
        self.keys(ctx);
        // The Piano Roll says again this frame whether the pointer is on it.
        self.st.piano_hover = false;
        // How fast frames really arrive (CPU and GPU together). In Auto, a
        // device that keeps missing the HUD's pace gets the lighter look.
        let now = ctx.input(|i| i.time);
        let gap = (now - self.last_frame).clamp(0.0, 1.0) as f32;
        self.last_frame = now;
        let busy_ms = self.frame_cpu_ms;
        let animating = self.st.theme.motion > 0.0 && self.st.visuals_mode != 2;
        if animating && gap < 0.5 {
            self.pace = self.pace * 0.97 + gap.max(busy_ms / 1000.0) * 0.03;
            self.paced_frames += 1;
        }
        if self.st.visuals_mode == 0
            && !self.st.visuals_lite
            && self.paced_frames > 120
            && self.pace > 0.055
        {
            self.st.visuals_lite = true;
            self.st
                .toast("Visual effects reduced for this device (View > Visual Effects)");
        }
        let lite = self.st.visuals_mode == 2 || (self.st.visuals_mode == 0 && self.st.visuals_lite);
        let mut th = self.st.theme.clone();
        if lite {
            th.grid_strength = 0.0;
            th.scanlines = 0.0;
            th.motion = 0.0;
            th.glow = th.glow.min(0.6);
        }
        let peak = self.st.meter.0.max(self.st.meter.1);
        self.st.fx.update(ctx, &th, peak);
        let fx = self.st.fx.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("af27-fx"), fx.clone()));
        let screen = ctx.screen_rect();
        // The living backdrop, then last frame's cards as glass on top of it.
        {
            let bg = ctx.layer_painter(egui::LayerId::background());
            crate::hud::backdrop(&bg, screen, &th, &fx);
            for r in &self.cards {
                crate::hud::glass(&bg, *r, &th);
                // Tab strips on the theme's marble.
                let strip = egui::Rect::from_min_max(r.min, egui::pos2(r.right(), r.top() + 26.0));
                crate::skin::bar(&bg, strip, &th);
            }
        }
        let frame = |fill: [u8; 3]| {
            egui::Frame::none()
                .fill(crate::theme::ca(fill, (th.glass * 200.0) as u8))
                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
        };

        // The theme's columns run the full height, so they come first.
        crate::skin::columns(ctx, &th);
        if th.checker.is_some() {
            egui::TopBottomPanel::top("skin-checker")
                .exact_height(crate::skin::checker_height(&th))
                .show_separator_line(false)
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    crate::skin::checker(ui.painter(), ui.max_rect(), &th)
                });
        }
        let under = egui::vec2(6.0, 4.0);
        egui::TopBottomPanel::top("menu")
            .frame(frame(th.background))
            .show(ctx, |ui| {
                crate::skin::bar(ui.painter(), ui.max_rect().expand2(under), &th);
                ui.horizontal(|ui| {
                    crate::skin::title(ui, &th);
                    menus(self, ui);
                });
            });
        if !th.columns.is_empty() {
            egui::TopBottomPanel::top("skin-meander")
                .exact_height(crate::skin::MEANDER)
                .show_separator_line(false)
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    crate::skin::meander(ui.painter(), ui.max_rect(), &th)
                });
        }
        egui::TopBottomPanel::top("tools")
            .frame(frame(th.background))
            .show(ctx, |ui| {
                crate::skin::bar(ui.painter(), ui.max_rect().expand2(under), &th);
                toolbar(ui, &mut self.st)
            });
        egui::TopBottomPanel::bottom("status")
            .frame(frame(th.background))
            .show(ctx, |ui| {
                crate::skin::bar(ui.painter(), ui.max_rect().expand2(under), &th);
                status(ui, &self.st)
            });

        let mut style = Style::from_egui(ctx.style().as_ref());
        let clear = egui::Color32::TRANSPARENT;
        style.dock_area_padding = Some(egui::Margin::same(4.0));
        style.main_surface_border_stroke = egui::Stroke::NONE;
        style.tab_bar.bg_fill = clear;
        style.tab_bar.height = 26.0;
        style.tab_bar.rounding = Rounding::ZERO;
        style.tab_bar.hline_color = crate::theme::ca(th.border, 120);
        for t in [
            &mut style.tab.active,
            &mut style.tab.inactive,
            &mut style.tab.focused,
            &mut style.tab.hovered,
            &mut style.tab.active_with_kb_focus,
            &mut style.tab.inactive_with_kb_focus,
            &mut style.tab.focused_with_kb_focus,
        ] {
            t.bg_fill = clear;
            t.outline_color = clear;
            t.rounding = Rounding::ZERO;
        }
        style.tab.active.bg_fill = crate::theme::ca(th.accent, 14);
        style.tab.focused.bg_fill = crate::theme::ca(th.accent, 24);
        style.tab.active.text_color = c(th.text);
        style.tab.focused.text_color = crate::theme::lerp(th.accent, [255, 255, 255], 0.3);
        style.tab.inactive.text_color = c(th.text_dim);
        style.tab.hovered.text_color = c(th.text);
        style.tab.hovered.bg_fill = crate::theme::ca(th.accent, 10);
        if let Some(t) = th.tab_active {
            style.tab.active.text_color = c(t);
            style.tab.focused.text_color = c(t);
        }
        style.tab.tab_body.bg_fill = clear;
        style.tab.tab_body.stroke = egui::Stroke::NONE;
        style.tab.tab_body.rounding = Rounding::ZERO;
        style.separator.color_idle = clear;
        style.separator.color_hovered = crate::theme::ca(th.accent, 160);
        style.separator.color_dragged = c(th.accent_alt);
        style.separator.width = 6.0;
        style.overlay.selection_color = crate::theme::ca(th.accent, 60);

        let mut open_tab = None;
        let mut tab_rects = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let mut viewer = Viewer {
                    st: &mut self.st,
                    caches: &mut self.viewer,
                    open_tab: &mut open_tab,
                    tab_rects: &mut tab_rects,
                    crowded: &self.crowded,
                    showing: &self.showing,
                };
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_inside(ui, &mut viewer);
                // Where dividers meet, drag both at once.
                crate::corners::corners(ui, &mut self.dock);
                // The HUD over the cards: edges, brackets, the running light
                // on the focused card and the light under each active tab.
                let p = ui.painter();
                let focused = self.dock.focused_leaf().map(|(_, n)| n.0);
                self.cards.clear();
                self.crowded.clear();
                self.showing.clear();
                let mut serial = 0;
                for (i, node) in self.dock.main_surface().iter().enumerate() {
                    if let egui_dock::Node::Leaf {
                        rect, tabs, active, ..
                    } = node
                    {
                        serial += 1;
                        let on = focused == Some(i);
                        self.cards.push(*rect);
                        // Would the full titles fit? If not, this panel's
                        // tabs go compact next frame.
                        let need: f32 = tabs
                            .iter()
                            .map(|t| {
                                let title = if *t == Tab::Editor {
                                    "Editor: a file name.wav"
                                } else {
                                    t.title()
                                };
                                full_tab_width(ui.ctx(), title)
                            })
                            .sum();
                        if need > rect.width() - 8.0 {
                            self.crowded.extend(tabs.iter().copied());
                        }
                        if let Some(t) = tabs.get(active.0) {
                            self.showing.insert(*t);
                        }
                        crate::hud::frame(p, *rect, &th, &fx, on, serial);
                        if tabs.get(active.0) == Some(&Tab::Editor) {
                            crate::skin::lane_art(p, *rect, &th);
                        }
                        if let Some(t) = tabs.get(active.0) {
                            if let Some((_, r)) = tab_rects.iter().find(|(x, _)| x == t) {
                                let r = r.intersect(rect.shrink2(egui::vec2(1.0, 0.0)));
                                if r.width() > 2.0 {
                                    crate::hud::tab_light(p, r, &th, &fx, on);
                                }
                            }
                        }
                    }
                }
            });
        if let Some(t) = open_tab {
            self.show_tab(t);
        }
        // Dropped files the Multitrack did not take open (folders searched).
        if let Some(d) = self.st.sessions.os_drop.take() {
            let (audio, sessions) = crate::sessions::expand_paths(&d.paths);
            for s in sessions {
                self.st.open_session(&s);
            }
            for a in audio {
                self.st.load_file(a, None);
            }
        }
        crate::sessions::windows(ctx, &mut self.st);
        if std::mem::take(&mut self.st.reset_workspace) {
            self.dock = self.st.workspace.build();
        }
        if let Some(w) = self.st.want_workspace.take() {
            self.switch_workspace(w);
        }
        if self.st.want_piano {
            self.st.want_piano = false;
            self.show_tab(Tab::PianoRoll);
        }
        generate_window(ctx, &mut self.st);
        crate::export::poll(&mut self.st, ctx);
        crate::export::show(ctx, &mut self.st);
        crate::tempo::window(ctx, &mut self.st);
        crate::tempo::replace_prompt(ctx, &mut self.st);
        about_window(ctx, &mut self.st);
        self.settings.show(ctx, &mut self.st);
        let th_now = self.st.theme.clone();
        self.shortcuts.show(ctx, &mut self.keymap, &th_now);
        {
            let top = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("af27-scan"),
            ));
            crate::hud::scanlines(&top, screen, &th, &fx);
        }
        crate::hud::boot(ctx, &th, &fx, &mut self.boot_skipped);
        crate::pointers::draw(ctx, &self.st.theme.pointers);
        self.frame_cpu_ms = __t0.elapsed().as_secs_f32() * 1000.0;
        if self.st.is_playing()
            || self.st.is_recording()
            || !self.st.jobs.is_empty()
            || self.st.separation.job.is_some()
            || self.st.spectral_job.is_some()
            || self.st.meter.0 > 0.001
        {
            ctx.request_repaint();
        } else if fx.animating(&th)
            && ctx.input(|i| {
                i.viewport().focused.unwrap_or(true) && !i.viewport().minimized.unwrap_or(false)
            })
        {
            // The HUD keeps moving at a gentle 30 frames a second when idle,
            // only while the window has focus: in the background it rests.
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

/// Shared helpers for panels.
pub fn meter_db(x: f32) -> f32 {
    gain_to_db(x).max(-90.0)
}
