// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! The window: menu bar, tool strip, the dockable panel area, transport
//! and status bar. Every panel lives in the dock and can be dragged to a
//! new place, tabbed with others, split, or floated in its own window.

use egui::{RichText, Rounding};
use egui_dock::{DockArea, DockState, Style, TabViewer};
use serde::{Deserialize, Serialize};

use crate::keymap::{Cmd, Keymap};
use crate::state::{data_dir, Mode, State, Tool};
use crate::theme::c;
use af2_core::time::{format_time, gain_to_db};
use af2_dsp::Effect;

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
    Properties,
    Diagnostics,
    Loudness,
    Video,
}

impl Tab {
    pub const ALL: [Tab; 20] = [
        Tab::Properties,
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
        Tab::Diagnostics,
        Tab::Loudness,
        Tab::Video,
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
            Tab::Properties => "Props",
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
            Tab::Properties => "Properties",
            Tab::Space => "3D Space",
            Tab::PianoRoll => "Piano Roll",
            Tab::Visualizer => "Visualizer",
            Tab::Diagnostics => "Diagnostics",
            Tab::Loudness => "Loudness",
            Tab::Video => "Video",
        }
    }
}

/// A tab's title: spaced capitals, or tighter and smaller when its panel
/// is crowded.
fn tab_text(t: &str, compact: bool, plain: bool) -> egui::WidgetText {
    if plain {
        // As a 1995 program titled its tabs: as written, unspaced.
        return RichText::new(t)
            .size(if compact { 10.0 } else { 11.0 })
            .into();
    }
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
    /// When the last frame started (the low-latency frame cap).
    frame_started: Option<std::time::Instant>,
    pub st: State,
    dock: DockState<Tab>,
    pub viewer: Caches,
    /// The docked cards as drawn last frame: their glass goes under them.
    cards: Vec<egui::Rect>,
    crowded: std::collections::HashSet<Tab>,
    shown_env_tab: bool,
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
    let saved: Option<DockState<Tab>> = std::fs::read_to_string(layout_path(w))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    match saved {
        Some(mut d) => {
            // Once: the Video panel joins an arrangement saved before it
            // existed, behind History (closing it later is kept).
            let mark = data_dir().join(format!("layout-video-{}", w.key()));
            if !mark.exists() {
                crate::workspace::add_behind(&mut d, Tab::History, Tab::Video);
                let _ = std::fs::write(&mark, b"");
            }
            d
        }
        None => w.build(),
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<std::path::PathBuf>) -> App {
        let mut st = State::new();
        st.live_metronome();
        // Once, after an install: offer to bring Audiform 1's setup over.
        crate::af1_import::ask_once(&mut st);
        // A firewall choice made in Settings follows to a new copy of the
        // app (not during a self-screenshot run).
        if std::env::var_os("AF2_SCREENSHOT").is_none() {
            crate::settings::firewall_follow(&mut st.prefs);
        }
        crate::hwcursor::install(cc);
        crate::wayland_dnd::install(cc);
        crate::caustics::install(cc);
        crate::app_icon::install(cc);
        lay_colors(&mut st);
        st.theme.apply(&cc.egui_ctx);
        crate::app_icon::apply(&cc.egui_ctx, &st.theme);
        st.theme_changed = false;
        st.open_paths(files);
        let dock = load_layout(st.workspace);
        App {
            frame_started: None,
            st,
            dock,
            viewer: Caches::default(),
            cards: Vec::new(),
            crowded: Default::default(),
            shown_env_tab: false,
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
        } else if let Some((s, n, _)) = self
            .dock
            .find_tab(&Tab::Editor)
            .filter(|_| matches!(tab, Tab::PianoRoll | Tab::Mixer))
        {
            // Closed: back beside the Editor, not in a window of its own.
            self.dock.set_focused_node_and_surface((s, n));
            self.dock.push_to_focused_leaf(tab);
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
        let plain = crate::retro::on(&self.st.theme);
        if *tab == Tab::Editor {
            let name = match self.st.mode {
                Mode::Waveform => self
                    .st
                    .doc()
                    .map(|d| format!("{}{}", d.name, if d.dirty { " *" } else { "" }))
                    .unwrap_or_else(|| "no file".into()),
                Mode::Multitrack if self.st.no_session() => "(none)".into(),
                Mode::Multitrack => format!(
                    "{}{}",
                    self.st.session.name,
                    if self.st.session_dirty { " *" } else { "" }
                ),
            };
            return tab_text(&format!("{}: {name}", t), self.crowded.contains(tab), plain);
        }
        if self.crowded.contains(tab) {
            return tab_text(tab.short_title(), true, plain);
        }
        tab_text(t, false, plain)
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        self.tab_rects.push((*tab, response.rect));
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let tt = std::time::Instant::now();
        self.ui_inner(ui, tab);
        if std::env::var_os("AF2_TRACE").is_some() && tt.elapsed().as_millis() > 20 {
            eprintln!("tab {:?} {:.1} ms", tab, tt.elapsed().as_secs_f64() * 1e3);
        }
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
            | Tab::Space
            // The rack fills the panel's height exactly and scrolls its own
            // rows; an outer bar would come and go every frame on the
            // rounding (Grey Desktop '95's solid bars made it shake).
            | Tab::Effects => [false, false],
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
            Tab::Effects => crate::rack::panel(ui, self.st),
            Tab::AiSeparation => crate::separation::show(ui, self.st),
            Tab::History => crate::panels::history(ui, self.st),
            Tab::Levels => crate::panels::levels(ui, self.st),
            Tab::Phase => crate::phasewheel::panel(ui, self.st, &mut self.caches.wheel),
            Tab::Statistics => crate::panels::statistics(ui, self.st),
            Tab::Spectrum => crate::panels::spectrum(ui, self.st),
            Tab::Selection => crate::panels::range(ui, self.st),
            Tab::Inspector => crate::inspector::panel(ui, self.st),
            Tab::Properties => crate::clip_props::panel(ui, self.st),
            Tab::Space => crate::space::panel(ui, self.st, &mut self.caches.space_drag),
            Tab::PianoRoll => crate::pianoroll::panel(ui, self.st, &mut self.caches.piano),
            Tab::Visualizer => crate::visualizer::show(ui, self.st),
            Tab::Diagnostics => crate::diagnostics::panel(ui, self.st),
            Tab::Loudness => crate::loudness_meter::panel(ui, self.st),
            Tab::Video => crate::video::panel(ui, self.st),
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
        // Ctrl+D in the Piano Roll (pointer over it, or clicked last)
        // duplicates the picked notes, whatever the key does elsewhere.
        if !typing
            && self.st.piano_hover
            && self.st.piano_clip.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D))
        {
            self.st.piano_cmd = Some(crate::pianoroll::PianoCmd::Duplicate);
        }
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
            Cmd::NewFile => crate::new_dialogs::open_new_file(st),
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
            // After a click in the Files panel, Delete closes the picked
            // files (as their menu's Close does).
            Cmd::Delete if st.files_focus && !st.files_ui.selected.is_empty() => {
                let picked: Vec<_> = st.files_ui.selected.clone();
                crate::files_panel::close_files(st, picked);
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
            Cmd::DuplicateClips => {
                if st.mode == Mode::Multitrack && !crate::clip_ops::duplicate(st) {
                    st.toast("Select clips to duplicate first");
                }
            }
            Cmd::Invert => apply_now(st, Effect::Invert),
            Cmd::Reverse => apply_now(st, Effect::Reverse),
            Cmd::Silence => apply_now(st, Effect::Silence),
            Cmd::MatchLoudness => {
                // It has a target to set, so it opens in the Effects panel.
                st.effect = Effect::MatchLoudness;
                st.effect_dialog = true;
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
            Cmd::ZoomFull => zoom_out_full(st),
            Cmd::ZoomSession => zoom_session(st),
            Cmd::ZoomSelection => zoom_selection(st),
            Cmd::ZoomInVertical => zoom_tracks(st, crate::zoom::TRACK_KEY_STEP),
            Cmd::ZoomOutVertical => zoom_tracks(st, 1.0 / crate::zoom::TRACK_KEY_STEP),
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
                    st.effect_dialog = true;
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

/// Zoom In / Zoom Out (keys, menu, buttons; twice or half the time per
/// press): the playhead keeps its place, or the middle of the view when
/// the playhead is out of view.
pub fn zoom(st: &mut State, f: f64) {
    let pos = st.position();
    st.hand_moved_view();
    match st.mode {
        Mode::Waveform => {
            let lim = st.doc_limits();
            if let Some(d) = st.doc_mut() {
                let anchor = crate::zoom::button_anchor(&d.view, pos);
                d.view.zoom_at(f, anchor, &lim);
            }
        }
        Mode::Multitrack => {
            let lim = st.session_limits();
            let anchor = crate::zoom::button_anchor(&st.session_view, pos);
            st.session_view.zoom_at(f, anchor, &lim);
        }
    }
}

/// Zoom to Selection: the selection fills the view (a little room on
/// each side in the Multitrack, 5 % before it in the Waveform editor).
pub fn zoom_selection(st: &mut State) {
    let Some((a, b)) = st.selection() else { return };
    st.hand_moved_view();
    match st.mode {
        Mode::Waveform => {
            let lim = st.doc_limits();
            if let Some(d) = st.doc_mut() {
                d.view = crate::state::View::show_range(a, b, 0.05, &lim);
            }
        }
        Mode::Multitrack => {
            let lim = st.session_limits();
            st.session_view = crate::state::View::show_range(a, b, 0.025, &lim);
        }
    }
}

/// Zoom to Session: the whole session in view with 5 % room after it, or
/// in the Waveform editor exactly the whole file.
pub fn zoom_session(st: &mut State) {
    st.hand_moved_view();
    match st.mode {
        Mode::Waveform => {
            let lim = st.doc_limits();
            if let Some(d) = st.doc_mut() {
                d.view = crate::state::View::fit_room(d.duration(), 1.0, &lim);
            }
        }
        Mode::Multitrack => {
            let lim = st.session_limits();
            let end = st.session.duration();
            st.session_view = crate::state::View::fit_room(end.max(10.0), 1.05, &lim);
        }
    }
}

/// Zoom Out Full: in the Multitrack every track fitted into the lanes at
/// one height (one undo step), then Zoom to Session; in the Waveform
/// editor the whole file.
pub fn zoom_out_full(st: &mut State) {
    if st.mode == Mode::Multitrack {
        let n = st.session.tracks.len();
        if n > 0 {
            let h = crate::zoom::fit_height(st.session_lanes_h, crate::multitrack::TAIL_H, n);
            let before = st.session.clone();
            for t in &mut st.session.tracks {
                t.height = h;
            }
            if before != st.session {
                st.commit_session("Track Height", before);
            }
        }
    }
    zoom_session(st);
}

/// Taller Tracks / Shorter Tracks: every track times `f`, each keeping
/// its own size, as one undo step.
pub fn zoom_tracks(st: &mut State, f: f32) {
    let before = st.session.clone();
    for t in &mut st.session.tracks {
        t.height = (t.height * f).clamp(crate::zoom::TRACK_MIN_H, crate::zoom::TRACK_MAX_H);
    }
    if before != st.session {
        st.commit_session("Track Height", before);
    }
}

/// Amplitude In / Out (display only): twice or half as tall.
pub fn zoom_amp(st: &mut State, notches: f64) {
    match st.mode {
        Mode::Waveform => {
            if let Some(d) = st.doc_mut() {
                d.amp = crate::zoom::wheel_amp(d.amp, notches);
            }
        }
        Mode::Multitrack => st.session_amp = crate::zoom::wheel_amp(st.session_amp, notches),
    }
}

const AUDIO_FILTER: &[&str] = af2_io::EXTENSIONS;

pub fn open_dialog(st: &mut State) {
    let mut all: Vec<&str> = AUDIO_FILTER.to_vec();
    all.extend(["af2", "af27", "sesx", "audfrm", "altaudio", "altemplate", "aup3", "otio", crate::archive::EXT]);
    if let Some(paths) = crate::file_dialog::FileDialog::new()
        .add_filter("Audio and sessions", &all)
        .add_filter("Audio", AUDIO_FILTER)
        .add_filter(
            "Sessions",
            &["af2", "af27", "sesx", "audfrm", "altaudio", "altemplate", "aup3", "otio", crate::archive::EXT],
        )
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
    if let Some(paths) = crate::file_dialog::FileDialog::new()
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

const PLUGIN_NOTE: &str =
    "VST 2 plug-ins are not supported (Steinberg no longer licenses its SDK). Use VST 3.";

fn menus(app: &mut App, ui: &mut egui::Ui) {
    let st = &mut app.st;
    let km = &app.keymap;
    let mut open_tab = None;
    let mut cmd: Option<Cmd> = None;
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            cmd_item(ui, km, Cmd::NewFile, "New Audio File", true, &mut cmd);
            if ui
                .button("New Multitrack Session...")
                .on_hover_text("The session open now is set aside, not closed")
                .clicked()
            {
                crate::new_dialogs::open_new_session(st, None);
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
                        crate::new_dialogs::open_new_session(st, Some(t.clone()));
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
                    if let Some(p) = crate::file_dialog::FileDialog::new()
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
            if ui
                .button("Archive Session...")
                .on_hover_text("The session and all the audio it uses in one small file that opens anywhere")
                .clicked()
            {
                ui.close_menu();
                st.archive.open = true;
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
                if ui.button("Multitrack Session (.sesx)...").clicked() {
                    crate::export::export_sesx(st);
                    ui.close_menu();
                }
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
            crate::lane_menu::multitrack_menu_top(ui, st);
            if ui
                .button("Insert Session as Clip...")
                .on_hover_text("Another session's whole mix as one clip, at the playhead on the picked track. It follows that session's saves.")
                .clicked()
            {
                ui.close_menu();
                crate::linked::insert_dialog(st);
            }
            playback_menu_items(ui, st, km, &mut cmd);
            ui.menu_button("Spoken Word", |ui| {
                if ui
                    .button("Add Podcast Tracks")
                    .on_hover_text("Host and Guest tracks with a voice clean-up chain, plus Music Bed and Sound Effects")
                    .clicked()
                {
                    crate::podcast_tools::add_podcast_tracks(st);
                    ui.close_menu();
                }
                ui.separator();
                if ui
                    .button("Auto Duck...")
                    .on_hover_text("Dip music and effects tracks under the talking with volume automation")
                    .clicked()
                {
                    st.podcast.duck_open = true;
                    ui.close_menu();
                }
                if ui
                    .button("Strip Silence...")
                    .on_hover_text("Split clips where they go quiet and take the quiet parts out")
                    .clicked()
                {
                    st.podcast.strip_open = true;
                    ui.close_menu();
                }
                if ui
                    .button("Speaker Balance...")
                    .on_hover_text("Bring every voice track to the same loudness with clip gain")
                    .clicked()
                {
                    st.podcast.balance_open = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("Export", |ui| {
                if ui.button("MIDI File...").clicked() {
                    ui.close_menu();
                    if let Some(p) = crate::file_dialog::FileDialog::new()
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
            cmd_item(
                ui,
                km,
                Cmd::DuplicateClips,
                "Duplicate Selected Clips",
                st.selected_clip.is_some(),
                &mut cmd,
            );
            if ui.button("Delete Selected Clip").clicked() {
                st.delete_clip();
                ui.close_menu();
            }
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
            ui.separator();
            if ui.button("Session Properties...").clicked() {
                open_props(st);
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
            for c in [
                Cmd::ZoomIn,
                Cmd::ZoomOut,
                Cmd::ZoomSession,
                Cmd::ZoomSelection,
                Cmd::ZoomFull,
                Cmd::ZoomInVertical,
                Cmd::ZoomOutVertical,
            ] {
                let enabled = match c {
                    Cmd::ZoomSelection => st.selection().is_some(),
                    Cmd::ZoomInVertical | Cmd::ZoomOutVertical => st.mode == Mode::Multitrack,
                    _ => true,
                };
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(c.label()).shortcut_text(km.text(c)),
                    )
                    .clicked()
                {
                    cmd = Some(c);
                }
            }
            ui.menu_button("Time Display", |ui| time_display_items(ui, st));
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
                            .add(crate::widgets::Slider::new(value, 0.0..=359.0).suffix("°").text(label))
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
                for (head, idx) in crate::theme::grouped(&st.themes) {
                    // A hidden theme is listed only while it is in use.
                    let shown: Vec<usize> = idx
                        .into_iter()
                        .filter(|&i| !st.themes[i].hidden || st.theme_base.name == st.themes[i].name)
                        .collect();
                    if shown.is_empty() {
                        continue;
                    }
                    if let Some(h) = head {
                        ui.separator();
                        ui.label(RichText::new(h).small().color(c(st.theme.text_dim)));
                    }
                    for i in shown {
                        let n = st.themes[i].name.clone();
                        let on = st.theme_base.name == n;
                        let label = if st.themes[i].hidden { "Secret Theme" } else { n.as_str() };
                        let mut r = ui.radio(on, label);
                        if crate::theme::is_system(&st.themes[i]) {
                            r = r.on_hover_text("Your desktop's colors, light or dark, following its settings as they change");
                        } else if st.themes[i].author == "Audiform" && !st.themes[i].hidden {
                            r = r.on_hover_text("From the original Audiform");
                        }
                        if r.clicked() {
                            let t = st.themes[i].clone();
                            st.set_theme(t);
                            let _ = std::fs::create_dir_all(data_dir());
                            let _ = std::fs::write(data_dir().join("theme.txt"), &n);
                            ui.close_menu();
                        }
                    }
                }
                ui.separator();
                if ui.button("Save Current Theme As...").clicked() {
                    let dir = data_dir().join("themes");
                    let _ = std::fs::create_dir_all(&dir);
                    if let Some(p) = crate::file_dialog::FileDialog::new()
                        .set_directory(&dir)
                        .set_file_name(format!("{}.af2theme", st.theme.name))
                        .add_filter("AudiForm 2 theme", &["af2theme"])
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
                ui.add(crate::widgets::Slider::new(&mut st.theme.glow, 0.0..=2.0).text("Glow"));
                if ui
                    .add(crate::widgets::Slider::new(&mut st.theme.rounding, 0.0..=14.0).text("Rounding"))
                    .changed()
                {
                    st.theme_changed = true;
                }
                if ui
                    .add(crate::widgets::Slider::new(&mut st.theme.font_scale, 0.8..=1.4).text("Text size"))
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
                ui.add(crate::widgets::Slider::new(&mut st.theme.grid_strength, 0.0..=2.0).text("Grid"));
                ui.add(crate::widgets::Slider::new(&mut st.theme.glass, 0.3..=1.0).text("Glass"));
                ui.add(crate::widgets::Slider::new(&mut st.theme.chamfer, 0.0..=20.0).text("Corner cut"));
                ui.add(crate::widgets::Slider::new(&mut st.theme.scanlines, 0.0..=0.6).text("Scan lines"));
                ui.add(crate::widgets::Slider::new(&mut st.theme.motion, 0.0..=2.0).text("Motion"));
                ui.add(crate::widgets::Slider::new(&mut st.theme.reactive, 0.0..=1.5).text("Reacts to music"));
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
            if ui.button("Report a Problem...").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(
                    "https://github.com/AudiFormer/AudiForm2/issues/new",
                ));
                ui.close_menu();
            }
            if ui.button("Check for Updates...").clicked() {
                st.updater.check(true);
                ui.close_menu();
            }
            crate::desktop_entry::menu_item(ui, st);
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
    // The selected clip's own rack, in the Effects panel's Clip tab.
    let clip_track = st
        .selected_clip
        .and_then(|id| st.session.find_clip(id))
        .map(|(ti, _)| st.session.tracks[ti].id);
    if ui
        .add_enabled(
            multi && clip_track.is_some(),
            egui::Button::new("Edit Clip Effects in Effects Rack"),
        )
        .on_hover_text("The clip's own effects, which run before its track's")
        .on_disabled_hover_text("Select a clip in the Multitrack editor")
        .clicked()
    {
        st.selected_track = clip_track;
        st.rack_tab = crate::rack::RackTab::Clip;
        *open_tab = Some(Tab::Effects);
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
        if ui.button("Noise...").clicked() {
            st.generate.noise_open = true;
            ui.close_menu();
        }
        if ui.button("Tones...").clicked() {
            st.generate.tones_open = true;
            ui.close_menu();
        }
        if ui.button("Silence (5 s)").clicked() {
            let (rate, ch) = st
                .doc()
                .map_or((48_000, 2), |d| (d.buf.rate, d.buf.num_channels()));
            st.insert_buffer("Insert silence", af2_dsp::generate::silence(rate, ch, 5.0));
            ui.close_menu();
        }
    });
    ui.menu_button("Spectrum", |ui| {
        crate::spectral_select::effects_submenu(ui, st)
    });
    ui.separator();
    cmd_item(ui, km, Cmd::MatchLoudness, "Match Loudness", true, cmd);
    if ui
        .add_enabled(has_doc, egui::Button::new("Capture Noise Print"))
        .on_hover_text("Learn the background noise from the selection (select only noise)")
        .on_disabled_hover_text(no_doc)
        .clicked()
    {
        crate::noise_print::capture(st);
        ui.close_menu();
    }
    if ui
        .add_enabled(has_doc, egui::Button::new("Reduce Noise with Print..."))
        .on_disabled_hover_text(no_doc)
        .clicked()
    {
        st.noise_print.open = true;
        ui.close_menu();
    }
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
                            if !add_to_track_rack(st, e.realtime().then(|| e.id().to_string())) {
                                st.effect = e;
                                if show {
                                    st.effect_dialog = true;
                                }
                            }
                        }
                        crate::fx_menu::Item::Fx(id, _) => {
                            if !add_to_track_rack(st, Some(id.clone())) {
                                st.fx_pick = Some(id);
                                if show {
                                    st.effect_dialog = true;
                                }
                            }
                        }
                        crate::fx_menu::Item::Stems => *open_tab = Some(Tab::AiSeparation),
                        crate::fx_menu::Item::Spatialize => st.spatialize.open = true,
                        crate::fx_menu::Item::Diagnose(k) => {
                            if st.diag.kind != k {
                                st.diag.kind = k;
                                st.diag.found.clear();
                                st.diag.scanned = None;
                            }
                            *open_tab = Some(Tab::Diagnostics);
                        }
                    }
                    ui.close_menu();
                }
            }
        });
    }
    ui.separator();
    disabled_submenu(ui, "VST", PLUGIN_NOTE);
    ui.menu_button("VST 3", |ui| {
        if let Some(id) = crate::plugins::picker_items(ui) {
            if !add_to_track_rack(st, Some(id.clone())) && st.file_chain.len() < 16 {
                // The Waveform editor: into the file's rack.
                st.file_chain.push(crate::rack::new_slot(&id));
                let at = st.file_chain.len() - 1;
                crate::rack::open_window(st, crate::rack::FILE, at);
                st.want_effects = true;
            }
        }
    });
    if ui.button("Audio Plug-In Manager...").clicked() {
        crate::plugins::open_manager();
        ui.close_menu();
    }
}

/// In the Multitrack, an effect picked from the Effects menu goes into the
/// selected track's rack (and opens). False when it did not (Waveform, no
/// track, or an effect that cannot run live).
fn add_to_track_rack(st: &mut State, id: Option<String>) -> bool {
    let Some(id) = id else { return false };
    if st.mode != Mode::Multitrack {
        return false;
    }
    let Some(ti) = st.selected_track.and_then(|t| st.session.track_index(t)) else {
        return false;
    };
    if st.session.tracks[ti].chain.len() >= af2_core::session::CHAIN_SLOTS {
        st.toast("This track's rack is full (16 effects).");
        return true;
    }
    let before = st.session.clone();
    st.session.tracks[ti].chain.push(crate::rack::new_slot(&id));
    let at = st.session.tracks[ti].chain.len() - 1;
    crate::rack::open_window(st, ti as u16, at);
    st.commit_session("Add effect", before);
    true
}

/// The Effects menu's own window: one effect applied to the file.
fn effect_dialog(ctx: &egui::Context, st: &mut State) {
    if !st.effect_dialog {
        return;
    }
    let mut open = true;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "ui.rs", 0)), "Effect")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(340.0)
        .show(ctx, |ui| crate::panels::effects(ui, st));
    st.effect_dialog = open;
}

/// A tool button with a drawn icon: Move (a four-way arrow), Razor (a
/// blade over a dashed cut line) and Time Selection (an I-beam).
fn tool_button(
    ui: &mut egui::Ui,
    tool: Tool,
    on: bool,
    th: &crate::theme::Theme,
) -> egui::Response {
    use egui::{pos2, vec2, Stroke};
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 24.0), egui::Sense::click());
    let p = ui.painter_at(rect.expand(1.0));
    let hot = resp.hovered();
    if th.bevel.is_some() {
        p.rect_filled(
            rect,
            0.0,
            if on {
                crate::theme::lerp(th.panel_raised, th.accent, 0.25)
            } else {
                c(th.panel_raised)
            },
        );
        crate::skin::bevel(&p, rect, th, on || resp.is_pointer_button_down_on());
    } else if on || hot {
        p.rect_filled(
            rect,
            3.0,
            if on {
                crate::theme::ca(th.accent, 60)
            } else {
                crate::theme::ca(th.text, 16)
            },
        );
    }
    let col = if on {
        c(th.accent)
    } else if hot {
        c(th.text)
    } else {
        crate::theme::lerp(th.text, th.text_dim, 0.3)
    };
    let s = Stroke::new(1.6_f32, col);
    let m = rect.center();
    match tool {
        Tool::Move => {
            for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let tip = m + vec2(dx, dy) * 7.5;
                p.line_segment([m + vec2(dx, dy) * 1.5, tip], s);
                let side = vec2(-dy, dx) * 2.6;
                let back = tip - vec2(dx, dy) * 2.8;
                p.add(egui::Shape::convex_polygon(
                    vec![tip, back + side, back - side],
                    col,
                    Stroke::NONE,
                ));
            }
        }
        Tool::Razor => {
            // The cut: a dashed vertical line.
            let mut y = rect.top() + 3.0;
            while y < rect.bottom() - 3.0 {
                p.line_segment(
                    [pos2(m.x, y), pos2(m.x, (y + 2.5).min(rect.bottom() - 3.0))],
                    Stroke::new(1.0_f32, col),
                );
                y += 4.5;
            }
            // The blade, tilted across it: a slim parallelogram with its
            // sharp edge lit.
            let a = pos2(m.x - 8.0, m.y + 3.0);
            let b = pos2(m.x + 6.0, m.y - 5.0);
            let up = vec2(1.6, 2.8);
            let blade = vec![a, b, b + up, a + up];
            p.add(egui::Shape::convex_polygon(
                blade,
                crate::theme::ca(th.background, 200),
                s,
            ));
            p.line_segment([a, b], Stroke::new(2.0_f32, col));
        }
        Tool::BoxSelect => {
            // A dashed rectangle.
            let r = egui::Rect::from_center_size(m, vec2(17.0, 13.0));
            let thin = Stroke::new(1.4_f32, col);
            let corners = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ];
            for i in 0..4 {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let len = (b - a).length();
                let dir = (b - a) / len;
                let mut d = 0.0;
                while d < len {
                    p.line_segment([a + dir * d, a + dir * (d + 3.0).min(len)], thin);
                    d += 5.0;
                }
            }
        }
        Tool::LassoSelect => {
            // A freeform loop with a tail hanging from it.
            let c0 = m + vec2(1.5, -2.5);
            let pts: Vec<egui::Pos2> = (0..=40)
                .map(|i| {
                    let a = i as f32 / 40.0 * std::f32::consts::TAU;
                    let wob = 1.0 + 0.12 * (a * 3.0).sin();
                    c0 + vec2(a.cos() * 8.0, a.sin() * 5.0) * wob
                })
                .collect();
            p.add(egui::Shape::line(pts, s));
            let tail = [
                c0 + vec2(-5.5, 3.6),
                m + vec2(-6.5, 4.5),
                m + vec2(-4.5, 7.0),
                m + vec2(-7.5, 8.5),
            ];
            p.add(egui::Shape::line(tail.to_vec(), s));
            p.circle_filled(tail[3], 1.5, col);
        }
        Tool::BrushSelect => {
            // A brush: handle, ferrule and a soft tip.
            let dir = vec2(1.0, -1.0).normalized();
            let side = vec2(-dir.y, dir.x);
            let tip = m + vec2(-7.0, 7.0);
            let neck = tip + dir * 6.0;
            let top = m + vec2(8.0, -8.0);
            p.line_segment([neck + dir * 3.0, top], Stroke::new(2.2_f32, col));
            p.add(egui::Shape::convex_polygon(
                vec![
                    neck + side * 2.2,
                    neck + dir * 3.0 + side * 1.6,
                    neck + dir * 3.0 - side * 1.6,
                    neck - side * 2.2,
                ],
                col,
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![
                    neck + side * 2.4,
                    neck - side * 2.4,
                    tip - side * 0.6 - dir * 0.5,
                    tip + side * 0.8,
                ],
                crate::theme::ca(th.background, 120),
                s,
            ));
        }
        Tool::SpotRepair => {
            // A spot healer: a ring with a centre dot.
            p.circle_stroke(m, 7.5, s);
            p.circle_filled(m, 2.0, col);
            for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                let d = vec2(dx, dy);
                p.line_segment([m + d * 7.5, m + d * 10.0], Stroke::new(1.2_f32, col));
            }
        }
        _ => {
            // I-beam.
            p.line_segment([pos2(m.x, m.y - 7.0), pos2(m.x, m.y + 7.0)], s);
            for y in [m.y - 7.0, m.y + 7.0] {
                p.line_segment([pos2(m.x - 3.5, y), pos2(m.x + 3.5, y)], s);
            }
        }
    }
    resp
}

/// A popup window that opens dead centre every time it opens (and can be
/// dragged while open). `key` tells one popup from another: a window not
/// drawn last frame has just opened, so it is put back in the middle.
pub fn popup<'a>(
    ctx: &egui::Context,
    key: egui::Id,
    title: impl Into<egui::WidgetText>,
) -> egui::Window<'a> {
    let frame = ctx.cumulative_pass_nr();
    let last: Option<u64> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, frame));
    let centre = ctx.screen_rect().center();
    let w = egui::Window::new(title)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(centre);
    if last.is_none_or(|l| l + 1 < frame) {
        w.current_pos(centre)
    } else {
        w
    }
}

/// Every node of the main dock surface, to adjust tab bar scrolls.
fn viewer_dock_nodes(dock: &mut DockState<Tab>) -> impl Iterator<Item = &mut egui_dock::Node<Tab>> {
    dock.main_surface_mut().iter_mut()
}

/// The Harmonics setting, applied at once to the selection there is.
fn set_harmonics(st: &mut State, set: crate::spectral_select::HarmSet) {
    use crate::spectral_select::{HARM_ALL, HARM_OFF};
    // Never none at all: unticking the last one leaves the shape itself.
    let set = if set & HARM_ALL == 0 { HARM_OFF } else { set };
    st.harmonics = set;
    if let Some(d) = st.doc_mut() {
        let ny = d.buf.rate as f64 / 2.0;
        d.spec_sel.set_harmonics(set, ny);
    }
}

/// The toolbar's name for a Harmonics set: "off", "all", "1, 3, 5".
fn harmonics_label(set: crate::spectral_select::HarmSet) -> String {
    use crate::spectral_select::{HARM_ALL, HARM_OFF};
    if set == HARM_OFF {
        return "Harmonics: off".into();
    }
    if set & HARM_ALL == HARM_ALL {
        return "Harmonics: all".into();
    }
    let odd: u64 = (1..64).step_by(2).map(|k| 1u64 << k).sum();
    let even: u64 = (2..64).step_by(2).map(|k| 1u64 << k).sum();
    if set == odd {
        return "Harmonics: odd".into();
    }
    if set == even {
        return "Harmonics: even".into();
    }
    let ks: Vec<String> = (1..=10).filter(|k| set & (1u64 << k) != 0).map(|k| k.to_string()).collect();
    let more = if set >> 11 != 0 { ", ..." } else { "" };
    format!("Harmonics: {}{more}", ks.join(", "))
}

/// The spectral selection tools (Waveform editor only) and the brush size.
fn spectral_tools(ui: &mut egui::Ui, st: &mut State) {
    ui.separator();
    let wave = st.mode == Mode::Waveform;
    let th = st.theme.clone();
    for (tool, tip) in [
        (
            Tool::BoxSelect,
            "Box Select: drag a rectangle in the spectrum (Shift adds)",
        ),
        (
            Tool::LassoSelect,
            "Lasso Select: draw around a sound in the spectrum (Shift adds)",
        ),
        (
            Tool::BrushSelect,
            "Brush Select: paint over a sound in the spectrum (Shift adds)",
        ),
        (
            Tool::SpotRepair,
            "Spot Repair: paint over a sound; it is repaired when you let go",
        ),
    ] {
        let on = st.tool == tool;
        if ui
            .add_enabled_ui(wave, |ui| tool_button(ui, tool, on, &th))
            .inner
            .on_hover_text(tip)
            .on_disabled_hover_text("Spectral tools work in the Waveform editor")
            .clicked()
        {
            st.choose_tool(tool);
        }
    }
    // Harmonic selection: each shape also copied to whole multiples of its
    // frequency, whichever of them are ticked.
    {
        use crate::spectral_select::{HARM_ALL, HARM_OFF};
        let label = harmonics_label(st.harmonics);
        ui.add_enabled_ui(wave, |ui| {
            egui::menu::menu_custom_button(
                ui,
                egui::Button::new(RichText::new(label).small()).selected(st.harmonics != HARM_OFF),
                |ui| {
                    ui.label(
                        RichText::new("Copies of each selection at whole multiples of its frequency")
                            .small()
                            .color(c(th.text_dim)),
                    );
                    let mut set = st.harmonics;
                    for k in 1..=10u32 {
                        let mut on = set & (1u64 << k) != 0;
                        let text = match k {
                            1 => "1  (the shape you draw)".to_string(),
                            2 => "2  (an octave up)".to_string(),
                            k => format!("{k}"),
                        };
                        if ui.checkbox(&mut on, text).changed() {
                            set = if on { set | (1u64 << k) } else { set & !(1u64 << k) };
                        }
                    }
                    ui.separator();
                    let odd: u64 = (1..64).step_by(2).map(|k| 1u64 << k).sum();
                    let even: u64 = (2..64).step_by(2).map(|k| 1u64 << k).sum();
                    ui.horizontal(|ui| {
                        if ui.button("All").on_hover_text("Every harmonic up to the top of the spectrum").clicked() {
                            set = HARM_ALL;
                        }
                        if ui.button("Odd").on_hover_text("1, 3, 5, 7... (as a clarinet's)").clicked() {
                            set = odd;
                        }
                        if ui.button("Even").on_hover_text("2, 4, 6, 8... (leaving out the shape itself)").clicked() {
                            set = even;
                        }
                        if ui.button("Off").on_hover_text("Just the shape you draw").clicked() {
                            set = HARM_OFF;
                        }
                    });
                    if set != st.harmonics {
                        set_harmonics(st, set);
                    }
                },
            )
            .response
            .on_hover_text(
                "Harmonic selection: copies of each shape you draw with Box, Lasso or Brush Select, the same size, \
                 moved to 2, 3, 4... times its frequency, to take a sound's overtones with it. Tick the ones you want; \
                 changing them updates the selection there is",
            )
        });
    }
    if wave && matches!(st.tool, Tool::BrushSelect | Tool::SpotRepair) {
        ui.add(
            crate::widgets::Slider::new(&mut st.brush_px, 4.0..=120.0)
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
        match crate::hud::segmented(ui, "af2-mode", &["Waveform", "Multitrack"], cur, &th, &fx) {
            Some(0) => st.mode = Mode::Waveform,
            Some(1) => st.mode = Mode::Multitrack,
            _ => {}
        }
        ui.add_space(14.0);
        for (tool, tip) in [
            (Tool::Move, "Move (V)"),
            (Tool::Razor, "Razor (R)"),
            (Tool::TimeSelect, "Time Selection (T)"),
        ] {
            if tool_button(ui, tool, st.tool == tool, &th)
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
                .selected_text(if crate::retro::on(&th) {
                    RichText::new(st.workspace.name())
                } else {
                    RichText::new(st.workspace.name().to_uppercase())
                        .size(11.0)
                        .extra_letter_spacing(1.2)
                        .color(c(th.accent))
                })
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
            if crate::retro::on(&th) {
                ui.label("Workspace:");
            } else {
                ui.label(
                    RichText::new("WORKSPACE")
                        .size(9.5)
                        .extra_letter_spacing(1.5)
                        .color(c(th.text_dim)),
                );
            }
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
        // In a glide it shows the tempo there and is edited in the Tempo
        // window instead.
        let seg = st.session.tempo.seg_at_secs(st.position().max(0.0));
        let (mut bpm, glide) = crate::tempo::playhead_bpm(st);
        let r = ui
            .add_enabled_ui(!glide, |ui| {
                ui.add_sized(
                    egui::vec2(78.0, 26.0),
                    egui::DragValue::new(&mut bpm)
                        .range(af2_core::midi::BPM_MIN..=af2_core::midi::BPM_MAX)
                        .speed(0.5)
                        .suffix(" BPM")
                        .max_decimals(if glide { 1 } else { 2 }),
                )
            })
            .inner;
        let r = if glide {
            r.on_disabled_hover_text(crate::tempo::glide_text(st))
        } else {
            r.on_hover_text("Tempo where the playhead is: drag, or double-click to type")
        };
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
        // The metronome is switched on above the track list (it shows as
        // its own row there).
        // The transport sits in the middle of the bar (its width measured
        // last frame), never closer than 14 points to what is left of it.
        let group_id = egui::Id::new("af2-transport-group-w");
        let group_w: f32 = ui.data(|d| d.get_temp(group_id)).unwrap_or(330.0);
        let bar = ui.max_rect();
        let want = bar.center().x - group_w / 2.0;
        ui.add_space((want - ui.cursor().left()).max(14.0));
        let group_start = ui.cursor().left();
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
        crate::record_input::default_input(ui, &mut st.rec_inputs);
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
        let measured = ui.cursor().left() - group_start;
        if (measured - group_w).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(group_id, measured));
            ui.ctx().request_repaint();
        }
        // A surround file: how it is heard (also Preferences > Playback).
        if st.mode == Mode::Waveform {
            surround_button(ui, st);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right to left: Zoom Out Full, Zoom to Selection, Zoom Out,
            // Zoom In, then the amplitude and (Multitrack) track heights.
            let multitrack = st.mode == Mode::Multitrack;
            for (s, tip, which) in [
                ("⛶", "Zoom Out Full (|)", 0u8),
                ("[ ]", "Zoom to Selection", 1),
                ("−", "Zoom Out (-)", 2),
                ("+", "Zoom In (=)", 3),
                ("A−", "Amplitude Out (display only)", 4),
                ("A+", "Amplitude In (display only)", 5),
                ("H−", "Shorter Tracks (Alt+-)", 6),
                ("H+", "Taller Tracks (Alt+=)", 7),
            ] {
                if which >= 6 && !multitrack {
                    continue;
                }
                if ui
                    .button(RichText::new(s).size(if which >= 4 { 12.0 } else { 16.0 }))
                    .on_hover_text(tip)
                    .clicked()
                {
                    match which {
                        0 => zoom_out_full(st),
                        1 => zoom_selection(st),
                        2 => zoom(st, 2.0),
                        3 => zoom(st, 0.5),
                        4 => zoom_amp(st, -1.0),
                        5 => zoom_amp(st, 1.0),
                        6 => zoom_tracks(st, 1.0 / crate::zoom::TRACK_KEY_STEP),
                        _ => zoom_tracks(st, crate::zoom::TRACK_KEY_STEP),
                    }
                }
            }
        });
    });
}

/// The playback mode of a file of more than two channels, on the
/// transport: a menu of the modes, Auto showing what it chose.
fn surround_button(ui: &mut egui::Ui, st: &mut State) {
    use crate::settings::SurroundMode;
    use af2_engine::SurroundPlay;
    let Some(n) = st.doc().map(|d| d.buf.num_channels()).filter(|n| *n > 2) else {
        return;
    };
    let fits = st.surround_direct_fits();
    let now = st.surround_play();
    let heard = match now {
        SurroundPlay::Fold => "Stereo fold",
        SurroundPlay::Headphones => "Headphones",
        SurroundPlay::Speakers => "Speakers",
        SurroundPlay::Direct if fits => "Direct outs",
        SurroundPlay::Direct => "Direct (folded)",
    };
    let mode = st.prefs.surround_play;
    let text = if mode == SurroundMode::Auto {
        format!("{n} ch · Auto: {heard}")
    } else {
        format!("{n} ch · {heard}")
    };
    ui.add_space(10.0);
    let r = ui
        .menu_button(RichText::new(text).small(), |ui| {
            for m in SurroundMode::ALL {
                let tip = if m == SurroundMode::Direct && !fits {
                    "Needs Settings > Channel Routing's speaker outputs on, and a device with                      enough outputs; until then it plays folded to stereo"
                } else {
                    m.describe()
                };
                if ui
                    .radio(st.prefs.surround_play == m, m.label())
                    .on_hover_text(tip)
                    .clicked()
                {
                    st.prefs.surround_play = m;
                    st.prefs.save();
                    ui.close_menu();
                }
            }
        })
        .response;
    r.on_hover_text(format!(
        "How this {n}-channel file plays: {} (Preferences > Playback)",
        mode.describe()
    ));
}

/// A panel's caption under the `win95` chrome: its shown tab's title, with
/// the file or session name for the editor.
fn caption_title(st: &State, tab: Tab) -> String {
    if tab != Tab::Editor {
        return tab.title().to_string();
    }
    let name = match st.mode {
        Mode::Waveform => st
            .doc()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "(Untitled)".into()),
        Mode::Multitrack => st.session.name.clone(),
    };
    format!("{name} - {}", tab.title())
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
            if let Some(p) = &t.reveal {
                if ui
                    .small_button("Show File")
                    .on_hover_text("Show it in the file manager")
                    .clicked()
                {
                    crate::sessions::reveal(p);
                }
            }
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

fn about_window(ctx: &egui::Context, st: &mut State) {
    let mut open = st.show_about;
    crate::ui::popup(ctx, egui::Id::new(("af2-popup", "ui.rs", 2)), "About AudiForm 2").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(120.0, 120.0), egui::Sense::hover());
        crate::hud::logo_mark(ui.painter(), r, &st.theme, &st.fx, 1.0);
        ui.label(RichText::new("AUDIFORM 2").size(28.0).strong().extra_letter_spacing(6.0).color(c(st.theme.accent)));
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label(RichText::new("Highly experimental alpha: expect bugs, and keep backups of sessions you care about.").strong());
        ui.hyperlink_to("Report a problem (GitHub Issues)", "https://github.com/AudiFormer/AudiForm2/issues");
        ui.label("A fast, themeable audio editor written entirely in Rust.");
        ui.label("Free software under the GNU General Public License, version 3 or later (see LICENSE and COPYING). The DTS decoder (af2-dts) is LGPL-2.1-or-later, ported from FFmpeg.");
        ui.label("The sequel to Audiform, by the same project lead.");
        if let Some(credit) = af2_engine::ASIO_CREDIT {
            ui.label(credit);
        }
    });
    st.show_about = open;
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::wayland_dnd::stop();
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Without vsync, frames are capped at 240 a second so steady
        // animation does not run the graphics card flat out.
        if self.st.prefs.low_latency {
            const MIN: std::time::Duration = std::time::Duration::from_micros(4_166);
            if let Some(t) = self.frame_started {
                let e = t.elapsed();
                if e < MIN {
                    std::thread::sleep(MIN - e);
                }
            }
            self.frame_started = Some(std::time::Instant::now());
        }
        crate::selfshot::tick(ctx, ctx.input(|i| i.time));
        crate::plugins::remember_window(frame);
        let pt = std::time::Instant::now();
        crate::plugins::tick(&mut self.st);
        crate::instruments::tick(&mut self.st);
        if std::env::var_os("AF2_TRACE").is_some() {
            eprintln!("plugins tick {:.1} ms", pt.elapsed().as_secs_f64() * 1e3);
        }
        // For documentation shots: AF2_SHOW_TAB=Inspector (etc.) opens that
        // panel once at start.
        if !self.shown_env_tab {
            self.shown_env_tab = true;
            // AF2_OPEN=new-file | new-session | multitrack | generate-noise |
            // generate-tones | overlap-demo | archive | spatialize | fx:<effect id>[?key=value&...] |
            // master-fx:<effect id>, for shots. AF2_FX_SET=key=value,... sets
            // the opened effect's controls (by key; `session_tempo` sets the
            // session's tempo); AF2_OPEN_DATA sets the slot's data (such as
            // an impulse response's file as JSON).
            match std::env::var("AF2_OPEN").as_deref() {
                Ok("new-file") => crate::new_dialogs::open_new_file(&mut self.st),
                Ok("new-session") => crate::new_dialogs::open_new_session(&mut self.st, None),
                Ok("multitrack") => self.st.mode = Mode::Multitrack,
                Ok("overlap-demo") => crate::overlap::demo(&mut self.st),
                Ok(v) if v.starts_with("video:") => {
                    // A session with this video dropped on its first track,
                    // and the Video panel in front, for shots.
                    let path = std::path::PathBuf::from(&v["video:".len()..]);
                    self.st.new_session(None);
                    self.st.mode = Mode::Multitrack;
                    if let Some(t) = self.st.session.tracks.first().map(|t| t.id) {
                        self.st.load_file(path, Some((t, 0.0)));
                    }
                    self.show_tab(Tab::Video);
                }
                Ok("session-clip") => {
                    // The first clip as a session clip, for shots.
                    crate::overlap::demo(&mut self.st);
                    if let Some(c) = self.st.session.tracks.iter().flat_map(|t| t.clips.first()).next() {
                        let m = c.media;
                        self.st.session.linked.insert(
                            m,
                            af2_core::session::LinkedSession { path: "Overdub base.audfrm".into(), stamp: String::new() },
                        );
                    }
                }
                Ok("archive") => {
                    crate::overlap::demo(&mut self.st);
                    self.st.archive.open = true;
                }
                Ok(v) if v.starts_with("instrument-menu") => {
                    crate::instruments::shot(&mut self.st, v)
                }
                Ok("generate-noise") => self.st.generate.noise_open = true,
                Ok("spatialize") => self.st.spatialize.open = true,
                // spatialize:<layout>: the open file made into that layout.
                Ok(v) if v.starts_with("spatialize:") => {
                    if let Some(l) = af2_core::space::SpaceLayout::from_label(&v[11..]) {
                        self.st.spatialize.layout = l;
                        self.st.spatialize.apply_when_open = true;
                    }
                }
                Ok("generate-tones") => self.st.generate.tones_open = true,
                // synth: the built-in synth's window on the first MIDI
                // track (one is added when there is none);
                // AF2_SYNTH_MODEL=<model name> picks its model.
                Ok("synth") => {
                    self.st.mode = Mode::Multitrack;
                    let id = match self
                        .st
                        .session
                        .tracks
                        .iter()
                        .find(|t| t.kind == af2_core::TrackKind::Midi)
                    {
                        Some(t) => t.id,
                        None => self.st.add_track_kind(af2_core::TrackKind::Midi),
                    };
                    if let Ok(m) = std::env::var("AF2_SYNTH_MODEL") {
                        if let Some(m) = af2_core::synth_model::SynthModel::ALL
                            .into_iter()
                            .find(|x| x.name().eq_ignore_ascii_case(m.trim()))
                        {
                            if let Some(t) = self.st.session.track_mut(id) {
                                t.instrument = m.preset();
                            }
                        }
                    }
                    self.st.synth_editor = Some(id);
                    // A shot's own setup: nothing to ask about on quit.
                    self.st.session_dirty = false;
                }
                // fx:<effect id>: that effect in the file's rack, open.
                Ok(v) if v.starts_with("fx:") => {
                    let (id, rest) = v[3..].split_once('?').unwrap_or((&v[3..], ""));
                    let mut slot = shot_slot(&mut self.st, id);
                    let carry: Vec<(String, f64)> = rest
                        .split('&')
                        .filter_map(|kv| {
                            let (k, x) = kv.split_once('=')?;
                            Some((k.to_string(), x.parse().ok()?))
                        })
                        .collect();
                    if !carry.is_empty() {
                        slot.values = crate::fx_windows::values_for(id, &carry);
                    }
                    if let Some(d) = std::env::var("AF2_OPEN_DATA")
                        .ok()
                        .filter(|d| !d.is_empty())
                    {
                        slot.data = Some(d);
                    }
                    self.st.file_chain.push(slot);
                    let at = self.st.file_chain.len() - 1;
                    crate::rack::open_window(&mut self.st, crate::rack::FILE, at);
                }
                // master-fx:<effect id>: in the Multitrack's master rack.
                Ok(v) if v.starts_with("master-fx:") => {
                    self.st.mode = Mode::Multitrack;
                    let slot = shot_slot(&mut self.st, &v[10..]);
                    self.st.session.master_chain.push(slot);
                    let at = self.st.session.master_chain.len() - 1;
                    crate::rack::open_window(&mut self.st, af2_engine::MASTER, at);
                }
                _ => {}
            }
            if let Ok(name) = std::env::var("AF2_SHOW_TAB") {
                if let Some(t) = Tab::ALL
                    .iter()
                    .find(|t| format!("{t:?}").eq_ignore_ascii_case(name.trim()))
                {
                    self.show_tab(*t);
                }
            }
        }
        shot_setup(&mut self.st);
        // For shots: AF2_SELECT_CLIP=n selects the session's nth clip (from
        // 1, across tracks) once there is one; a stretched one when n is 0.
        if let Ok(v) = std::env::var("AF2_SELECT_CLIP") {
            if self.st.selected_clip.is_none() {
                let n: usize = v.trim().parse().unwrap_or(1);
                let all: Vec<&af2_core::Clip> = self
                    .st
                    .session
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .collect();
                let pick = if n == 0 {
                    all.iter().find(|c| c.props.stretch.is_on()).copied()
                } else {
                    all.get(n - 1).copied()
                };
                if let Some(c) = pick {
                    self.st.selected_clip = Some(c.id);
                }
            }
        }
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
        // The System theme follows the desktop's colours as they change
        // (read on a background thread; only a finished reading lands here).
        if crate::theme::is_system(&self.st.theme_base) {
            crate::system_look::watch(ctx);
            if let Some(c) = crate::system_look::take_change() {
                let t = crate::theme::Theme::system_from(&c);
                if let Some(e) = self.st.themes.iter_mut().find(|e| crate::theme::is_system(e)) {
                    *e = t.clone();
                }
                self.st.show_theme(t);
            }
        }
        if self.st.theme_changed {
            lay_colors(&mut self.st);
            self.st.theme.apply(ctx);
            crate::app_icon::apply(ctx, &self.st.theme);
            self.st.theme_changed = false;
        }
        crate::app_icon::frame();
        crate::linux_desktop::frame(ctx, &self.st);
        crate::wayland_dnd::frame(ctx);
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
        let tick_t = std::time::Instant::now();
        self.st.tick();
        if std::env::var_os("AF2_TRACE").is_some() {
            eprintln!("tick total {:.1} ms", tick_t.elapsed().as_secs_f64() * 1e3);
        }
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
            th.caustics = 0.0;
            th.glow = th.glow.min(0.6);
        }
        crate::widgets::set_theme(ctx, &th);
        let peak = self.st.meter.0.max(self.st.meter.1);
        self.st.fx.update(ctx, &th, peak);
        let fx = self.st.fx.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("af2-fx"), fx.clone()));
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
        let retro = crate::retro::on(&th);
        let frame = |fill: [u8; 3]| {
            egui::Frame::none()
                .fill(if retro {
                    c(th.panel)
                } else {
                    crate::theme::ca(fill, (th.glass * 200.0) as u8)
                })
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
                if retro {
                    // A raised tool bar under an etched line.
                    crate::retro::panel(
                        ui.painter(),
                        ui.max_rect().expand2(under),
                        None,
                        crate::retro::Edge::ThinRaised,
                    );
                }
                toolbar(ui, &mut self.st)
            });
        egui::TopBottomPanel::bottom("status")
            .frame(frame(th.background))
            .show(ctx, |ui| {
                crate::skin::bar(ui.painter(), ui.max_rect().expand2(under), &th);
                if retro {
                    // The status line in a sunken field.
                    crate::retro::panel(
                        ui.painter(),
                        ui.max_rect().expand2(egui::vec2(3.0, 2.0)),
                        None,
                        crate::retro::Edge::ThinSunken,
                    );
                }
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
        if retro {
            // Grey tabs under each window's caption, black titles.
            style.tab_bar.height = crate::retro::TAB_BAR;
            style.tab_bar.hline_color = clear;
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
                t.text_color = crate::retro::DARK;
            }
            style.buttons.close_tab_color = crate::retro::DARK;
            style.buttons.close_tab_active_color = crate::retro::DARK;
            style.buttons.add_tab_bg_fill = crate::retro::LIGHT;
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
        let mut tab_shift = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let dock_from = crate::retro::next_shape(ui.ctx(), ui.layer_id());
                let mut viewer = Viewer {
                    st: &mut self.st,
                    caches: &mut self.viewer,
                    open_tab: &mut open_tab,
                    tab_rects: &mut tab_rects,
                    crowded: &self.crowded,
                    showing: &self.showing,
                };
                // The wheel over a tab bar slides its tabs sideways (the
                // dock only listens on the bar's empty part, which a
                // crowded bar has none of).
                let bar_h = style.tab_bar.height;
                let (hover, wheel) = ui.input(|i| (i.pointer.hover_pos(), i.smooth_scroll_delta));
                if let (Some(hp), true) = (hover, wheel != egui::Vec2::ZERO) {
                    for node in viewer_dock_nodes(&mut self.dock) {
                        if let egui_dock::Node::Leaf { rect, scroll, .. } = node {
                            let bar = egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(rect.width(), bar_h),
                            );
                            if bar.contains(hp) {
                                *scroll += wheel.y + wheel.x;
                                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                                break;
                            }
                        }
                    }
                }
                let dt = std::time::Instant::now();
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_inside(ui, &mut viewer);
                if std::env::var_os("AF2_TRACE").is_some() {
                    eprintln!("dock {:.1} ms", dt.elapsed().as_secs_f64() * 1e3);
                }
                let dock_to = crate::retro::next_shape(ui.ctx(), ui.layer_id());
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
                        // The Visuals picture fills its card: no serial over it.
                        let tag = if tabs.get(active.0) == Some(&Tab::Visualizer) { 0 } else { serial };
                        crate::hud::frame(p, *rect, &th, &fx, on, tag);
                        if retro {
                            let mine: Vec<egui::Rect> = tabs
                                .iter()
                                .filter_map(|t| {
                                    tab_rects.iter().find(|(x, _)| x == t).map(|(_, r)| *r)
                                })
                                .collect();
                            let shown = tabs.get(active.0);
                            let title = shown.map(|t| caption_title(&self.st, *t));
                            crate::retro::window(
                                p,
                                *rect,
                                &mine,
                                shown.and_then(|t| {
                                    tab_rects.iter().find(|(x, _)| x == t).map(|(_, r)| *r)
                                }),
                                title.as_deref().unwrap_or(""),
                                on,
                            );
                        }
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
                if retro {
                    tab_shift = Some(crate::retro::TabShift {
                        layer: ui.layer_id(),
                        from: dock_from,
                        to: dock_to,
                        bars: self
                            .cards
                            .iter()
                            .map(|r| {
                                egui::Rect::from_min_size(
                                    r.min,
                                    egui::vec2(r.width(), crate::retro::TAB_BAR),
                                )
                            })
                            .collect(),
                    });
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
        // Stretched clips render in the background: keep drawing until
        // they are in.
        if af2_engine::stretch::pending() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        if std::mem::take(&mut self.st.want_properties) {
            self.show_tab(Tab::Properties);
        }
        if std::mem::take(&mut self.st.want_effects) {
            self.show_tab(Tab::Effects);
        }
        crate::rack::windows(ctx, &mut self.st);
        crate::new_dialogs::show(ctx, &mut self.st);
        crate::podcast_tools::show(ctx, &mut self.st);
        crate::noise_print::show(ctx, &mut self.st);
        crate::spatialize_ui::show(ctx, &mut self.st);
        crate::af1_import::window(ctx, &mut self.st);
        crate::plugins::windows(ctx, &mut self.st);
        effect_dialog(ctx, &mut self.st);
        crate::generate_ui::windows(ctx, &mut self.st);
        crate::export::poll(&mut self.st, ctx);
        crate::export::show(ctx, &mut self.st);
        crate::archive::show(ctx, &mut self.st);
        crate::linked::frame(ctx, &mut self.st);
        crate::video::frame(ctx, &mut self.st);
        crate::tempo::window(ctx, &mut self.st);
        crate::tempo::edit_popup(ctx, &mut self.st);
        crate::tempo::replace_prompt(ctx, &mut self.st);
        crate::synth_ui::window(ctx, &mut self.st);
        about_window(ctx, &mut self.st);
        crate::updater::ui(ctx, &mut self.st);
        crate::desktop_entry::ui(ctx, &mut self.st);
        self.settings.show(ctx, &mut self.st);
        let th_now = self.st.theme.clone();
        self.shortcuts.show(ctx, &mut self.keymap, &th_now);
        {
            let top = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("af2-scan"),
            ));
            crate::hud::scanlines(&top, screen, &th, &fx);
        }
        crate::hud::boot(ctx, &th, &fx, &mut self.boot_skipped);
        crate::pointers::draw(ctx, &self.st.theme.pointers);
        if crate::retro::on(&self.st.theme) {
            crate::retro::restyle(ctx, tab_shift.as_ref());
        }
        self.frame_cpu_ms = __t0.elapsed().as_secs_f32() * 1000.0;
        if self.st.is_playing()
            || self.st.is_recording()
            || !self.st.jobs.is_empty()
            || self.st.separation.job.is_some()
            || self.st.spectral_job.is_some()
            || self.st.levels.active(&self.st.prefs.meters)
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

/// For documentation shots, once the first file is open:
/// `AF2_CHANNELS_OFF=2,5` switches those channels off (counting from 1),
/// and `AF2_SPECTRAL_BOX=t0,t1,hz0,hz1` draws a Box selection.
/// A new slot of `id` for a documentation shot, with the controls named
/// in `AF2_FX_SET` (key=value, comma separated) set.
fn shot_slot(st: &mut State, id: &str) -> af2_core::EffectSlot {
    let mut slot = crate::rack::new_slot(id);
    let keys: Vec<&str> =
        af2_fx::info(id).map_or_else(Vec::new, |i| i.params.iter().map(|p| p.key).collect());
    for pair in std::env::var("AF2_FX_SET").unwrap_or_default().split(',') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let Ok(v) = v.trim().parse::<f64>() else {
            continue;
        };
        if k.trim() == "session_tempo" {
            st.session.tempo.bpm = v;
        } else if let Some(i) = keys.iter().position(|x| *x == k.trim()) {
            if let Some(x) = slot.values.get_mut(i) {
                *x = v as f32;
            }
        }
    }
    slot
}

fn shot_setup(st: &mut State) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) || st.doc().is_none() {
        return;
    }
    DONE.store(true, Ordering::Relaxed);
    let nums =
        |v: &str| -> Vec<f64> { v.split(',').filter_map(|x| x.trim().parse().ok()).collect() };
    if let Ok(v) = std::env::var("AF2_CHANNELS_OFF") {
        if let Some(d) = st.doc_mut() {
            for k in nums(&v) {
                d.chans = d.chans.toggle((k as usize).saturating_sub(1));
            }
        }
    }
    // AF2_SPEC_VIEW=lo,hi,log_step: the spectrum's frequency range and scale.
    if let Ok(v) = std::env::var("AF2_SPEC_VIEW") {
        if let &[lo, hi, step] = &nums(&v)[..] {
            st.spec.range = Some((lo, hi));
            st.spec.log_step = step as u8;
        }
    }
    // AF2_HARMONICS=1,3,5 or all: the Harmonics setting (applies to the
    // boxes below).
    if let Ok(v) = std::env::var("AF2_HARMONICS") {
        st.harmonics = if v.trim().eq_ignore_ascii_case("all") {
            crate::spectral_select::HARM_ALL
        } else {
            v.split(',')
                .filter_map(|k| k.trim().parse::<u32>().ok())
                .filter(|k| (1..64).contains(k))
                .fold(0u64, |s, k| s | (1u64 << k))
        };
        if st.harmonics == 0 {
            st.harmonics = crate::spectral_select::HARM_OFF;
        }
    }
    // Several boxes: separated by ';'.
    for v in std::env::var("AF2_SPECTRAL_BOX").unwrap_or_default().split(';') {
        let ny = st.doc().map_or(24_000.0, |d| d.buf.rate as f64 / 2.0);
        let harmonics = st.harmonics;
        if let (&[t0, t1, f0, f1], Some(d)) = (&nums(v)[..], st.doc_mut()) {
            use crate::spectral_select::{hz_to_u, harmonics_of, Region, SPoint};
            let base = Region::Box {
                a: SPoint {
                    t: t0,
                    u: hz_to_u(f0),
                },
                b: SPoint {
                    t: t1,
                    u: hz_to_u(f1),
                },
            };
            let _ = harmonics_of;
            d.spec_sel.push(base);
            d.spec_sel.set_harmonics(harmonics, ny);
            st.choose_tool(Tool::BoxSelect);
        }
    }
}
