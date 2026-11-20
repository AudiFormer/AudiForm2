// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

//! The Settings window (Edit > Preferences...) and the Keyboard Shortcuts
//! window. Settings shows a list of pages on the left and the chosen
//! page on the right; nothing changes until Apply or OK, and Cancel
//! leaves everything as it was. Only settings that really do something
//! are shown: a page with none yet says what is coming.

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense};
use serde::{Deserialize, Serialize};

use crate::keymap::{Binding, Cmd, Keymap};
use crate::state::{data_dir, State};
use crate::theme::{c, ca, Hues, Rgb, Theme};
use crate::workspace::Workspace;

// ------------------------------------------------------------ preferences

/// How the window is drawn (read at start-up by `main.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Renderer {
    #[default]
    Wgpu,
    Glow,
}

/// The user's own colours laid over the theme's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub waveform: Option<Rgb>,
    pub playhead: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub selection: Option<Rgb>,
}

impl ColorOverrides {
    /// Puts the colours on `theme`; where there is no override the colour
    /// comes back from `fresh` (the theme as made, with the user's hues).
    pub fn apply(&self, theme: &mut Theme, fresh: &Theme) {
        theme.wave_peak = self.waveform.unwrap_or(fresh.wave_peak);
        theme.wave_centre = match self.waveform {
            Some(w) => [
                (w[0] as f32 * 0.55) as u8,
                (w[1] as f32 * 0.55) as u8,
                (w[2] as f32 * 0.55) as u8,
            ],
            None => fresh.wave_centre,
        };
        theme.playhead = self.playhead.unwrap_or(fresh.playhead);
        theme.accent = self.accent.unwrap_or(fresh.accent);
        theme.selection = self.selection.unwrap_or(fresh.selection);
    }
}

/// Settings saved in `prefs.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub renderer: Renderer,
    /// Return to Start on Stop: Stop sends the playhead back to where
    /// playback began (true) or leaves it where playback reached (false).
    /// Mirrors `State::return_on_stop`.
    pub space_returns: bool,
    /// Normal, Loop Playback or Skip Selection.
    pub play_mode: crate::transport::PlayMode,
    /// How the clock, the rulers and typed times show time.
    pub time_format: crate::transport::TimeFormat,
    /// Clicks and selection edges near a ruler mark land on it.
    pub snap_ruler: bool,
    /// Scroll the view along with the playhead while playing.
    pub follow_playhead: bool,
    pub autosave: bool,
    pub autosave_minutes: u32,
    /// Each save first copies the previous file into Backups.
    pub backups: bool,
    /// How many backups of each session to keep (1 to 50).
    pub backups_keep: u32,
    /// What happens to audio imported into a saved session.
    pub media_policy: crate::sessions::MediaPolicy,
    /// The session's media subfolder.
    pub media_folder: String,
    /// When saving, copy in audio that came into the session while it was
    /// outside the session folder.
    pub copy_on_save: bool,
    /// Bring the Effects panel up when an effect is picked from the menu.
    pub show_effects_on_pick: bool,
    pub colors: ColorOverrides,
    pub audio: AudioPrefs,
    /// What the Multitrack's track headers show.
    pub header_view: crate::track_views::HeaderView,
    /// Stretch handles on clip corners.
    pub stretch_handles: bool,
    /// The metronome's sound and level (kept here, not in sessions).
    pub metronome: af27_core::midi::MetronomeSettings,
    /// What New Audio File and New Multitrack Session start with.
    pub new_file: crate::new_dialogs::NewFileDefaults,
    pub new_session: crate::new_dialogs::NewSessionDefaults,
    /// Soloing a track or bus unsolos every other (Ctrl+click always
    /// does, whatever this says).
    pub solo_exclusive: bool,
    /// The MIDI input played on armed MIDI tracks is heard through their
    /// instrument.
    pub midi_monitor: bool,
}

/// Settings > Audio Devices and Settings > Channel Routing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioPrefs {
    /// The system's own driver, or ASIO.
    pub host: af27_engine::HostKind,
    /// The ASIO driver (None: the best guess among those installed).
    pub asio_driver: Option<String>,
    /// Play through this device (None: the system default). System
    /// driver only: on ASIO the driver is the device.
    pub output: Option<String>,
    /// Record from this device (None: the system default).
    pub input: Option<String>,
    /// Frames per audio buffer (None: the driver's own).
    pub buffer: Option<u32>,
    /// Sample rate to run the device at (None: the device's own).
    pub rate: Option<u32>,
    /// Which outputs the stereo mix and 3D speakers play on.
    pub map: af27_engine::OutputMap,
}

impl AudioPrefs {
    pub fn choice(&self) -> af27_engine::DeviceChoice {
        af27_engine::DeviceChoice {
            host: self.host,
            asio_driver: self.asio_driver.clone(),
            output: self.output.clone(),
            input: self.input.clone(),
            buffer: self.buffer,
            rate: self.rate,
            map: self.map.clone(),
        }
    }
}

/// What a device listing is for: the driver, the ASIO driver, and the
/// output whose rates and buffer sizes are shown.
type ScanKey = (af27_engine::HostKind, Option<String>, Option<String>);

fn scan_key(a: &AudioPrefs) -> ScanKey {
    match a.host {
        af27_engine::HostKind::System => (a.host, None, a.output.clone()),
        af27_engine::HostKind::Asio => (a.host, a.asio_driver.clone(), None),
    }
}

/// The devices found for one driver choice: listed when the page opens,
/// again whenever the driver changes (the lists follow it), and on
/// Refresh.
#[derive(Clone)]
struct Scan {
    key: ScanKey,
    list: af27_engine::DeviceList,
    /// The newest listing came back empty for the same driver (one busy
    /// for a moment, or just restarted); this is the last one that had
    /// devices.
    stale: bool,
}

fn has_devices(l: &af27_engine::DeviceList) -> bool {
    !l.outputs.is_empty() || !l.inputs.is_empty()
}

/// A new listing, keeping the previous one's devices when the new one is
/// empty for the same driver.
fn merge_scan(prev: Option<&Scan>, key: ScanKey, list: af27_engine::DeviceList) -> Scan {
    match prev {
        Some(p)
            if p.key.0 == key.0
                && p.key.1 == key.1
                && !has_devices(&list)
                && has_devices(&p.list) =>
        {
            let mut kept = p.list.clone();
            kept.error = list.error;
            Scan {
                key,
                list: kept,
                stale: true,
            }
        }
        _ => Scan {
            key,
            list,
            stale: false,
        },
    }
}

/// Something a page asks the window to do with the app.
#[derive(Clone, Copy, PartialEq)]
enum PageAct {
    RefreshDevices,
    ReloadThemes,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            renderer: Renderer::Wgpu,
            space_returns: false,
            play_mode: crate::transport::PlayMode::Normal,
            time_format: crate::transport::TimeFormat::MinSec,
            snap_ruler: true,
            follow_playhead: true,
            autosave: true,
            autosave_minutes: 2,
            backups: true,
            backups_keep: 10,
            media_policy: crate::sessions::MediaPolicy::Copy,
            media_folder: "Audio".into(),
            copy_on_save: true,
            show_effects_on_pick: true,
            colors: ColorOverrides::default(),
            audio: AudioPrefs::default(),
            header_view: Default::default(),
            stretch_handles: false,
            metronome: Default::default(),
            new_file: Default::default(),
            new_session: Default::default(),
            solo_exclusive: false,
            midi_monitor: true,
        }
    }
}

impl Prefs {
    fn path() -> std::path::PathBuf {
        data_dir().join("prefs.json")
    }

    pub fn load() -> Prefs {
        std::fs::read_to_string(Prefs::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Prefs::path(), s);
        }
    }
}

use crate::zoom::{WheelAnchor, ZoomPrefs};

// ------------------------------------------------------------ the window

const PAGES: [&str; 18] = [
    "General",
    "Appearance",
    "Audio Devices",
    "Channel Routing",
    "Autosave",
    "Session Media",
    "Editing",
    "Effects",
    "Audio Separation",
    "Cache and Files",
    "Markers",
    "Multitrack",
    "Clips",
    "Playback and Recording",
    "Spectrum Views",
    "Time Display",
    "Loudness",
    "Privacy",
];

/// What each page will hold that AudiForm 2 cannot do yet.
fn coming(page: &str) -> &'static [&'static str] {
    match page {
        "Editing" => &["The shape new fades start as: straight, or a curve smooth at both ends."],
        "Clips" => &["Clip auto-crossfade where clips overlap."],
        "Playback and Recording" => &[
            "Meter yellow and red levels.",
            "Recording latency offset, punch-in and pre-roll.",
        ],
        "Time Display" => &["The time format (decimal, samples, bars and beats, SMPTE)."],
        "Loudness" => &["The loudness region and target."],
        "Cache and Files" => {
            &["Cache sizes, and clearing the waveform drawings (they are rebuilt on demand)."]
        }
        _ => &[],
    }
}

/// The dialog's working copy: what Apply or OK will set.
#[derive(Clone)]
struct Draft {
    prefs: Prefs,
    zoom: ZoomPrefs,
    theme: String,
    hues: Option<Hues>,
    visuals: u8,
    intro: bool,
    realistic: bool,
    workspace: Workspace,
    devices: Option<Scan>,
    /// The layout Channel Routing shows speakers for.
    routing_layout: Option<af27_core::space::SpaceLayout>,
    act: Option<PageAct>,
}

impl Draft {
    fn from(st: &State, zoom: ZoomPrefs) -> Draft {
        let mut prefs = st.prefs.clone();
        prefs.space_returns = st.return_on_stop;
        Draft {
            prefs,
            zoom,
            theme: st.theme_base.name.clone(),
            hues: st.hues,
            visuals: st.visuals_mode,
            intro: st.theme.intro,
            realistic: st.theme.realistic,
            workspace: st.workspace,
            devices: None,
            routing_layout: None,
            act: None,
        }
    }

    /// Back to the defaults, for one page.
    fn reset_page(&mut self, page: &str, st: &State) {
        let d = Prefs::default();
        match page {
            "General" => {
                self.prefs.renderer = d.renderer;
                self.zoom = ZoomPrefs::default();
                self.workspace = Workspace::from_key("");
            }
            "Appearance" => {
                self.hues = None;
                self.visuals = 0;
                let base = st.themes.iter().find(|t| t.name == self.theme);
                self.intro = base.is_none_or(|t| t.intro);
                self.realistic = base.is_none_or(|t| t.realistic);
                self.prefs.colors = d.colors;
            }
            "Autosave" => {
                self.prefs.autosave = d.autosave;
                self.prefs.autosave_minutes = d.autosave_minutes;
                self.prefs.backups = d.backups;
                self.prefs.backups_keep = d.backups_keep;
            }
            "Session Media" => {
                self.prefs.media_policy = d.media_policy;
                self.prefs.media_folder = d.media_folder.clone();
                self.prefs.copy_on_save = d.copy_on_save;
            }
            "Effects" => self.prefs.show_effects_on_pick = d.show_effects_on_pick,
            "Audio Devices" => {
                // The channel map has its own page.
                let map = self.prefs.audio.map.clone();
                self.prefs.audio = d.audio.clone();
                self.prefs.audio.map = map;
            }
            "Channel Routing" => self.prefs.audio.map = d.audio.map.clone(),
            "Playback and Recording" => {
                self.prefs.space_returns = d.space_returns;
                self.prefs.follow_playhead = d.follow_playhead;
            }
            _ => {}
        }
    }

    fn apply(&self, st: &mut State) {
        if self.theme != st.theme_base.name {
            if let Some(t) = st.themes.iter().find(|t| t.name == self.theme).cloned() {
                st.set_theme(t);
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(data_dir().join("theme.txt"), &self.theme);
            }
        }
        if self.hues != st.hues {
            st.set_hues(self.hues);
        }
        for t in [&mut st.theme, &mut st.theme_base] {
            t.intro = self.intro;
            t.realistic = self.realistic;
        }
        if self.visuals != st.visuals_mode {
            st.visuals_mode = self.visuals;
            st.visuals_lite = false;
            let _ = std::fs::create_dir_all(data_dir());
            let _ = std::fs::write(data_dir().join("visuals.txt"), self.visuals.to_string());
        }
        if self.workspace != st.workspace {
            st.want_workspace = Some(self.workspace);
        }
        st.return_on_stop = self.prefs.space_returns;
        // The transport's own choices may have changed while the window
        // was open (they are not on its pages): keep them.
        let mut prefs = self.prefs.clone();
        prefs.play_mode = st.prefs.play_mode;
        prefs.time_format = st.prefs.time_format;
        prefs.snap_ruler = st.prefs.snap_ruler;
        prefs.header_view = st.prefs.header_view;
        prefs.stretch_handles = st.prefs.stretch_handles;
        prefs.metronome = st.prefs.metronome;
        prefs.new_file = st.prefs.new_file.clone();
        prefs.new_session = st.prefs.new_session.clone();
        if prefs != st.prefs {
            let reopen = prefs.audio != st.prefs.audio;
            st.prefs = prefs;
            st.prefs.save();
            if reopen {
                st.reopen_audio();
            }
        }
        st.theme_changed = true;
    }
}

#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    page: usize,
    draft: Option<Draft>,
}

impl SettingsUi {
    pub fn open(&mut self) {
        self.open = true;
        self.draft = None;
    }

    pub fn show(&mut self, ctx: &egui::Context, st: &mut State) {
        if !self.open {
            self.draft = None;
            return;
        }
        let th = st.theme.clone();
        let mut draft = self
            .draft
            .take()
            .unwrap_or_else(|| Draft::from(st, st.zoom));
        let mut window_open = true;
        let mut close = false;
        let mut apply = false;
        let size = vec2(820.0, 540.0)
            .min(ctx.screen_rect().size() - vec2(40.0, 60.0))
            .max(vec2(520.0, 360.0));
        egui::Window::new("Settings")
            .open(&mut window_open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(ctx.screen_rect().center())
            .show(ctx, |ui| {
                egui::TopBottomPanel::bottom("settings-foot")
                    .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 8.0)))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        hairline(ui, &th);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Reset This Page").clicked() {
                                draft.reset_page(PAGES[self.page], st);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let ok = egui::Button::new(
                                        RichText::new("   OK   ").strong().color(c(th.background)),
                                    )
                                    .fill(c(th.accent));
                                    if ui.add(ok).clicked() {
                                        apply = true;
                                        close = true;
                                    }
                                    if ui.button("Apply").clicked() {
                                        apply = true;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    });
                egui::SidePanel::left("settings-nav")
                    .resizable(false)
                    .exact_width(196.0)
                    .frame(egui::Frame::none())
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("settings-nav-scroll")
                            .show(ui, |ui| {
                                for (i, name) in PAGES.iter().enumerate() {
                                    if nav_item(ui, name, i == self.page, &th) {
                                        self.page = i;
                                    }
                                }
                            });
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().inner_margin(egui::Margin {
                        left: 18.0,
                        right: 6.0,
                        top: 2.0,
                        bottom: 0.0,
                    }))
                    .show_inside(ui, |ui| {
                        let name = PAGES[self.page];
                        ui.label(RichText::new(name).size(19.0).color(c(th.text)));
                        ui.add_space(4.0);
                        hairline(ui, &th);
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .id_salt("settings-page-scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                page(ui, name, &mut draft, st, &th);
                                let todo = coming(name);
                                if !todo.is_empty() {
                                    ui.add_space(10.0);
                                    for t in todo {
                                        note(ui, &th, &format!("Coming: {t}"));
                                    }
                                }
                            });
                    });
            });
        match draft.act.take() {
            Some(PageAct::RefreshDevices) => {
                ensure_scan(&mut draft, st, true);
            }
            Some(PageAct::ReloadThemes) => {
                st.themes = crate::theme::load_all(&crate::theme::user_dir());
            }
            None => {}
        }
        if apply {
            draft.apply(st);
            st.zoom = draft.zoom;
            st.zoom.save();
        }
        if close || !window_open {
            self.open = false;
            self.draft = None;
        } else {
            self.draft = Some(draft);
        }
    }
}

/// A page's settings.
fn page(ui: &mut egui::Ui, name: &str, d: &mut Draft, st: &State, th: &Theme) {
    match name {
        "General" => {
            group(ui, th, "Workspace", |ui| {
                egui::ComboBox::from_id_salt("settings-workspace")
                    .selected_text(d.workspace.name())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for w in Workspace::ALL {
                            ui.selectable_value(&mut d.workspace, w, w.name())
                                .on_hover_text(w.about());
                        }
                    });
                note(ui, th, "The arrangement of panels the window opens with. Each workspace keeps its own layout as you leave it.");
            });
            group(ui, th, "Mouse wheel", |ui| {
                ui.checkbox(
                    &mut d.zoom.wheel_zooms_waveform,
                    "The mouse wheel over the waveform zooms in time (Waveform editor; in the Multitrack the wheel zooms over the ruler and scrolls over the tracks)",
                );
                ui.horizontal(|ui| {
                    ui.label("Each wheel step zooms by");
                    ui.add(
                        egui::DragValue::new(&mut d.zoom.step_percent)
                            .range(2.0..=100.0)
                            .speed(0.5)
                            .max_decimals(0),
                    );
                    ui.label("% a step");
                });
            });
            group(ui, th, "Wheel zoom centers on", |ui| {
                for (a, label) in [
                    (
                        WheelAnchor::Pointer,
                        "The mouse pointer (as over the time ruler)",
                    ),
                    (WheelAnchor::Playhead, "The playhead"),
                ] {
                    ui.radio_value(&mut d.zoom.wheel_anchor, a, label);
                }
            });
            group(ui, th, "Draw the window with", |ui| {
                ui.radio_value(
                    &mut d.prefs.renderer,
                    Renderer::Wgpu,
                    "The graphics card, modern way (DirectX 12, Vulkan or Metal)",
                );
                ui.radio_value(
                    &mut d.prefs.renderer,
                    Renderer::Glow,
                    "OpenGL (for older or virtual graphics drivers)",
                );
                note(ui, th, "Takes effect the next time AudiForm 2 starts. If the modern way cannot start, OpenGL is used on its own.");
            });
        }
        "Appearance" => appearance(ui, d, st, th),
        "Audio Devices" => audio_devices(ui, d, st, th),
        "Channel Routing" => channel_routing(ui, d, st, th),
        "Autosave" => {
            group(ui, th, "Autosave", |ui| {
                ui.checkbox(
                    &mut d.prefs.autosave,
                    "Save a copy of an unsaved Multitrack session on its own",
                );
                ui.add_enabled_ui(d.prefs.autosave, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Every");
                        ui.add(
                            egui::DragValue::new(&mut d.prefs.autosave_minutes)
                                .range(1..=60)
                                .speed(0.2),
                        );
                        ui.label(if d.prefs.autosave_minutes == 1 {
                            "minute"
                        } else {
                            "minutes"
                        });
                    });
                });
                let p = data_dir().join("autosave");
                note(
                    ui,
                    th,
                    &format!(
                        "Every open session with unsaved changes gets its own copy in {}; your own files are never touched. Saving or closing a session deletes its copy. Copies left when AudiForm 2 did not close normally are offered at the next start.",
                        p.display()
                    ),
                );
            });
            group(ui, th, "Backups", |ui| {
                ui.checkbox(
                    &mut d.prefs.backups,
                    "Before each save, copy the session file as it was into a Backups folder beside it",
                );
                ui.add_enabled_ui(d.prefs.backups, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Keep the newest");
                        ui.add(
                            egui::DragValue::new(&mut d.prefs.backups_keep)
                                .range(1..=50)
                                .speed(0.2),
                        );
                        ui.label("backups of each session");
                    });
                });
                note(ui, th, "Backups are named after the session and the time it was saved (UTC). Only these are ever deleted.");
            });
        }
        "Session Media" => {
            use crate::sessions::MediaPolicy;
            group(ui, th, "Audio imported into a saved session", |ui| {
                ui.radio_value(
                    &mut d.prefs.media_policy,
                    MediaPolicy::Copy,
                    "Copy it into the session folder",
                );
                ui.radio_value(&mut d.prefs.media_policy, MediaPolicy::Ask, "Ask each time");
                ui.radio_value(
                    &mut d.prefs.media_policy,
                    MediaPolicy::Leave,
                    "Leave it where it is",
                );
                ui.horizontal(|ui| {
                    ui.label("Folder beside the session file");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.prefs.media_folder).desired_width(140.0),
                    );
                });
                ui.checkbox(
                    &mut d.prefs.copy_on_save,
                    "When saving, copy in audio that came in while it was outside the session folder",
                );
                note(ui, th, "The original is never moved or changed. Files already inside the session folder stay where they are. The same file imported again uses its copy; a different file with the same name gets a numbered name. File > Collect Session Media copies in everything the session uses at any time.");
            });
        }
        "Multitrack" => {
            group(ui, th, "Solo", |ui| {
                ui.checkbox(
                    &mut d.prefs.solo_exclusive,
                    "Exclusive solo: soloing a track or bus un-solos the others",
                );
                note(
                    ui,
                    th,
                    "Ctrl+click on S always solos that track only. Ctrl+Shift+click on M, S \
                     or R sets it on every track. Soloing a bus solos what feeds it.",
                );
            });
            group(ui, th, "MIDI", |ui| {
                ui.checkbox(
                    &mut d.prefs.midi_monitor,
                    "Hear what you play on armed MIDI tracks through their instrument",
                );
            });
        }
        "Effects" => {
            group(ui, th, "Effects menu", |ui| {
                ui.checkbox(
                    &mut d.prefs.show_effects_on_pick,
                    "Bring up the Effects panel when you pick an effect from the Effects menu",
                );
                note(
                    ui,
                    th,
                    "Off: the effect is chosen in the Effects panel, ready for when you go to it.",
                );
            });
        }
        "Audio Separation" => {
            group(ui, th, "Model folders", |ui| {
                info(ui, th, "AudiForm 2", &st.sep_dirs.own.display().to_string());
                if let Some(l) = &st.sep_dirs.legacy {
                    info(ui, th, "Audiform", &l.display().to_string());
                }
                note(
                    ui,
                    th,
                    "Stem separation models are looked for in these folders.",
                );
            });
        }
        "Cache and Files" => {
            group(ui, th, "Data folder", |ui| {
                let dir = data_dir();
                ui.label(RichText::new(dir.display().to_string()).monospace());
                note(ui, th, "Themes, layouts, keyboard shortcuts, these settings and autosaves are kept here.");
                ui.horizontal(|ui| {
                    if ui.button("Copy Path").clicked() {
                        ui.ctx().copy_text(dir.display().to_string());
                    }
                    if ui.button("Open Folder").clicked() {
                        let _ = std::fs::create_dir_all(&dir);
                        let opener = if cfg!(windows) {
                            "explorer"
                        } else if cfg!(target_os = "macos") {
                            "open"
                        } else {
                            "xdg-open"
                        };
                        let _ = std::process::Command::new(opener).arg(&dir).spawn();
                    }
                });
            });
        }
        "Playback and Recording" => {
            group(ui, th, "Stop (Space while playing)", |ui| {
                ui.radio_value(
                    &mut d.prefs.space_returns,
                    false,
                    "Leaves the playhead where playback reached",
                );
                ui.radio_value(
                    &mut d.prefs.space_returns,
                    true,
                    "Returns to where playback started (Return to Start on Stop, Shift+X)",
                );
            });
            group(ui, th, "While playing", |ui| {
                ui.checkbox(
                    &mut d.prefs.follow_playhead,
                    "Follow the playhead: the view scrolls along when it reaches the edge",
                );
            });
        }
        _ => {
            if coming(name).is_empty() {
                note(
                    ui,
                    th,
                    &format!("Settings for {} are coming.", name.to_lowercase()),
                );
            }
        }
    }
}

/// The device listing for the draft's driver choice, listing again when
/// the choice changed (or `force`). An ASIO driver other than the one
/// playing is not loaded just to list it (a process can hold one driver):
/// it is listed once Apply has switched to it.
fn ensure_scan(d: &mut Draft, st: &State, force: bool) -> Scan {
    use af27_engine::HostKind;
    let key = scan_key(&d.prefs.audio);
    let fresh = d.devices.as_ref().is_some_and(|s| s.key == key);
    if force || !fresh {
        let busy_with = st
            .engine
            .as_ref()
            .filter(|e| e.host == HostKind::Asio && key.0 == HostKind::Asio)
            .map(|e| e.device.clone());
        let wanted = (key.0 == HostKind::Asio)
            .then(|| {
                af27_engine::devices::pick_driver(
                    &af27_engine::devices::asio_drivers(),
                    key.1.as_deref(),
                )
            })
            .flatten();
        let list = match (busy_with, wanted) {
            (Some(open), Some(w)) if open != w => af27_engine::DeviceList {
                host: HostKind::Asio,
                driver: format!("ASIO ({w})"),
                asio_drivers: af27_engine::devices::asio_drivers(),
                asio_driver: Some(w.clone()),
                error: Some(format!(
                    "{open} is in use. Press Apply to switch to {w}; its inputs and outputs \
                     are listed once it is open."
                )),
                ..Default::default()
            },
            _ => af27_engine::devices::list(key.0, key.1.as_deref(), key.2.as_deref()),
        };
        d.devices = Some(merge_scan(d.devices.as_ref(), key, list));
    }
    d.devices.clone().unwrap_or(Scan {
        key: scan_key(&d.prefs.audio),
        list: Default::default(),
        stale: false,
    })
}

/// Settings > Audio Devices: the driver, what plays and records, the
/// sample rate and buffer, what is open now, and the audio log.
fn audio_devices(ui: &mut egui::Ui, d: &mut Draft, st: &State, th: &Theme) {
    use af27_engine::HostKind;
    let scan = ensure_scan(d, st, false);
    let list = &scan.list;
    let a = &mut d.prefs.audio;
    let row = |ui: &mut egui::Ui, label: &str| {
        ui.add_sized(vec2(118.0, 20.0), egui::Label::new(label));
    };
    let pick = |ui: &mut egui::Ui,
                id: &str,
                label: &str,
                cur: &mut Option<String>,
                all: &[af27_engine::DeviceInfo],
                default: &Option<String>,
                dir: &str| {
        let dflt = format!(
            "System default ({})",
            default.clone().unwrap_or_else(|| "none".into())
        );
        let shown = match cur {
            None => dflt.clone(),
            Some(n) if all.iter().any(|x| &x.name == n) => n.clone(),
            Some(n) => format!("{n} (not connected)"),
        };
        ui.horizontal(|ui| {
            row(ui, label);
            egui::ComboBox::from_id_salt(id)
                .selected_text(shown)
                .width(380.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(cur, None, dflt);
                    for x in all {
                        let text = format!("{}  ({} {dir})", x.name, x.channels);
                        ui.selectable_value(cur, Some(x.name.clone()), text);
                    }
                });
        });
    };
    let mut refresh = false;
    group(ui, th, "Audio hardware", |ui| {
        let system = format!("System ({})", af27_engine::driver_name());
        ui.horizontal(|ui| {
            row(ui, "Driver");
            let shown = match a.host {
                HostKind::System => system.clone(),
                HostKind::Asio if af27_engine::ASIO_BUILT => "ASIO".to_string(),
                HostKind::Asio => "ASIO (not in this build)".to_string(),
            };
            egui::ComboBox::from_id_salt("settings-driver")
                .selected_text(shown)
                .width(380.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.host, HostKind::System, system.as_str());
                    if af27_engine::ASIO_BUILT {
                        ui.selectable_value(&mut a.host, HostKind::Asio, "ASIO");
                    } else {
                        ui.add_enabled(
                            false,
                            egui::SelectableLabel::new(false, "ASIO (not in this build)"),
                        )
                        .on_disabled_hover_text(af27_engine::devices::NO_ASIO);
                    }
                });
        });
        if !af27_engine::ASIO_BUILT {
            let names = &list.asio_drivers;
            if !names.is_empty() {
                note(
                    ui,
                    th,
                    &format!(
                        "ASIO drivers on this computer: {}. This build of AudiForm 2 was made \
                         without ASIO, so it plays and records through {} instead.",
                        names.join(", "),
                        af27_engine::driver_name()
                    ),
                );
            }
        }
        if a.host == HostKind::Asio {
            ui.horizontal(|ui| {
                row(ui, "ASIO driver");
                let guess = af27_engine::devices::pick_driver(&list.asio_drivers, None);
                let auto = format!(
                    "Automatic ({})",
                    guess.unwrap_or_else(|| "none installed".into())
                );
                let shown = match &a.asio_driver {
                    None => auto.clone(),
                    Some(n) if list.asio_drivers.contains(n) => n.clone(),
                    Some(n) => format!("{n} (not installed)"),
                };
                egui::ComboBox::from_id_salt("settings-asio-driver")
                    .selected_text(shown)
                    .width(380.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.asio_driver, None, auto)
                            .on_hover_text("The interface's own driver, passing over wrappers such as ASIO4ALL and virtual cables");
                        for n in &list.asio_drivers {
                            ui.selectable_value(&mut a.asio_driver, Some(n.clone()), n);
                        }
                    });
            });
            match (list.outputs.first(), list.inputs.first()) {
                (None, None) => {}
                (o, i) => info(
                    ui,
                    th,
                    "Plays and records through",
                    &format!(
                        "{} ({} out, {} in)",
                        list.asio_driver.clone().unwrap_or_default(),
                        o.map_or(0, |x| x.channels),
                        i.map_or(0, |x| x.channels)
                    ),
                ),
            }
        } else {
            pick(
                ui,
                "settings-out",
                "Default output",
                &mut a.output,
                &list.outputs,
                &list.default_out,
                "out",
            );
            pick(
                ui,
                "settings-in",
                "Default input",
                &mut a.input,
                &list.inputs,
                &list.default_in,
                "in",
            );
        }
        ui.horizontal(|ui| {
            row(ui, "Sample rate");
            let own = format!(
                "Device's own{}",
                list.rate.map_or(String::new(), |r| format!(" ({r} Hz)"))
            );
            let shown = match a.rate {
                None => own.clone(),
                Some(r) if list.rates.is_empty() || list.rates.contains(&r) => format!("{r} Hz"),
                Some(r) => format!("{r} Hz (not offered)"),
            };
            egui::ComboBox::from_id_salt("settings-rate")
                .selected_text(shown)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.rate, None, own);
                    for &r in &list.rates {
                        ui.selectable_value(&mut a.rate, Some(r), format!("{r} Hz"));
                    }
                });
        });
        ui.horizontal(|ui| {
            row(ui, "Buffer size");
            let shown = match a.buffer {
                None => "Driver's own".to_string(),
                Some(n) => format!("{n} samples"),
            };
            egui::ComboBox::from_id_salt("settings-buffer")
                .selected_text(shown)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.buffer, None, "Driver's own");
                    for n in af27_engine::devices::buffer_choices(list.buffer_range) {
                        ui.selectable_value(&mut a.buffer, Some(n), format!("{n} samples"));
                    }
                });
            let rate = a.rate.or(st.engine.as_ref().map(|e| e.rate)).or(list.rate);
            if let (Some(n), Some(r)) = (a.buffer, rate) {
                ui.label(
                    RichText::new(format!("{:.1} ms", af27_engine::devices::latency_ms(n, r)))
                        .color(c(th.text_dim)),
                );
            }
        });
        if let Some((lo, hi)) = list.buffer_range {
            note(
                ui,
                th,
                &format!("The device takes buffers from {lo} to {hi} samples."),
            );
        }
        if let Some(e) = &list.error {
            ui.label(RichText::new(e).color(c(th.accent_alt)));
        }
        if scan.stale {
            note(ui, th, "The device list came back empty just now (a driver busy for a moment?), so the last list found is shown. Press Refresh Devices to look again.");
        }
        note(ui, th, match a.host {
            HostKind::System => "If a chosen device is unplugged, the system default plays instead, and the chosen one is used again once it is back and you press Apply. Tracks pick their own inputs, down to single channels, in the track header.",
            HostKind::Asio => "Only the chosen ASIO driver is loaded. Most drivers serve one program at a time: if the driver is busy, AudiForm 2 plays through the system driver and says so. Tracks pick the driver's channels in the track header.",
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Refresh Devices").clicked() {
                refresh = true;
            }
            if ui.button("Open System Sound Settings").clicked() {
                let _ = std::process::Command::new("explorer")
                    .arg("ms-settings:sound")
                    .spawn();
            }
        });
    });
    if refresh {
        d.act = Some(PageAct::RefreshDevices);
    }
    group(ui, th, "Open now", |ui| {
        match (&st.engine, &st.engine_error) {
            (Some(e), _) => {
                info(ui, th, "Playing through", &e.device);
                info(ui, th, "Driver", &e.driver);
                info(
                    ui,
                    th,
                    "Format",
                    &format!("{} Hz, {} channels, {:?}", e.rate, e.channels, e.format),
                );
                let asked = e
                    .buffer
                    .map_or("driver's own".to_string(), |n| format!("{n} samples"));
                let real = e.real_buffer().map_or(String::new(), |n| {
                    format!(
                        "; the device asks for {n} ({:.1} ms)",
                        af27_engine::devices::latency_ms(n, e.rate)
                    )
                });
                info(ui, th, "Buffer", &format!("{asked}{real}"));
                let session = st.session.rate;
                if session > 0 && session != e.rate {
                    note(ui, th, &format!("This session is {session} Hz and the device runs at {} Hz: playback is converted as it plays, and recordings are converted to the session's rate. Pick {session} Hz above to play it unconverted, if the device offers it.", e.rate));
                }
                if let Some(n) = &e.note {
                    ui.label(RichText::new(n).color(c(th.accent_alt)));
                }
                let lost = e.unplayable();
                if !lost.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "This device has no output for: {}. See Channel Routing.",
                            lost.join(", ")
                        ))
                        .color(c(th.accent_alt)),
                    );
                }
            }
            (None, Some(err)) => {
                ui.label(RichText::new(err).color(c(th.accent_alt)));
            }
            _ => note(ui, th, "No audio output is open."),
        }
    });
    group(ui, th, "Audio log", |ui| {
        let p = af27_engine::health::log_path().unwrap_or_else(|| data_dir().join("audio.log"));
        ui.label(RichText::new(p.display().to_string()).monospace());
        note(ui, th, "Each stream opened (driver, device, rate, channels, buffer), device errors and restarts are written here. Attach it when reporting an audio problem.");
        if ui.button("Copy Path").clicked() {
            ui.ctx().copy_text(p.display().to_string());
        }
    });
}

/// Settings > Channel Routing: which device outputs the stereo mix plays
/// on, and where each speaker of a 3D Space session goes.
fn channel_routing(ui: &mut egui::Ui, d: &mut Draft, st: &State, th: &Theme) {
    use af27_core::space::SpaceLayout;
    let scan = ensure_scan(d, st, false);
    let a = &d.prefs.audio;
    // Outputs the device offers: the chosen output's widest config.
    let listed = match a.host {
        af27_engine::HostKind::Asio => scan.list.outputs.first(),
        af27_engine::HostKind::System => a
            .output
            .as_deref()
            .and_then(|n| scan.list.output(n))
            .or_else(|| {
                scan.list
                    .default_out
                    .as_deref()
                    .and_then(|n| scan.list.output(n))
            }),
    };
    let width = listed
        .map(|x| usize::from(x.channels))
        .or(st.engine.as_ref().map(|e| e.channels))
        .unwrap_or(2)
        .max(1);
    let layout = *d
        .routing_layout
        .get_or_insert(st.session.space.unwrap_or(SpaceLayout::Surround714));
    let m = &mut d.prefs.audio.map;
    let out_name = |o: usize| format!("Output {}", o + 1);
    let output_box = |ui: &mut egui::Ui, id: String, cur: &mut u16| {
        let shown = if usize::from(*cur) < width {
            out_name(usize::from(*cur))
        } else {
            format!("{} (not on this device)", out_name(usize::from(*cur)))
        };
        egui::ComboBox::from_id_salt(id)
            .selected_text(shown)
            .width(170.0)
            .show_ui(ui, |ui| {
                for o in 0..width.max(usize::from(*cur) + 1) {
                    ui.selectable_value(cur, o as u16, out_name(o));
                }
            });
    };
    group(ui, th, "Stereo output", |ui| {
        for (side, k) in [("Left", 0), ("Right", 1)] {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(118.0, 20.0), egui::Label::new(side));
                output_box(ui, format!("route-stereo-{k}"), &mut m.stereo[k]);
            });
        }
        ui.horizontal(|ui| {
            ui.add_sized(vec2(118.0, 20.0), egui::Label::new(""));
            let mut p = 0;
            while p + 1 < width.min(16) {
                if ui
                    .small_button(format!("{}-{}", p + 1, p + 2))
                    .on_hover_text("Both sides to this pair")
                    .clicked()
                {
                    m.stereo = [p as u16, p as u16 + 1];
                }
                p += 2;
            }
        });
        note(ui, th, &format!(
            "Playback, hand-played notes and the metronome play on this pair. The device has {width} output{} as listed; outputs nothing is sent to stay silent. A mono device gets left and right summed.",
            if width == 1 { "" } else { "s" }
        ));
    });
    group(ui, th, "3D Space speakers", |ui| {
        ui.checkbox(
            &mut m.speakers_out,
            "Send each speaker of a 3D Space session to its own output (for a speaker rig)",
        );
        note(ui, th, "Off: 3D sessions are heard on the stereo pair, binaurally or folded (as chosen in the 3D Space panel). On: the device opens with all its outputs; meters and the Phase Wheel still show left and right.");
        ui.add_enabled_ui(m.speakers_out, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(118.0, 20.0), egui::Label::new("Layout"));
                egui::ComboBox::from_id_salt("route-layout")
                    .selected_text(layout.label())
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for l in SpaceLayout::ALL {
                            ui.selectable_value(&mut d.routing_layout, Some(l), l.label());
                        }
                    });
                if st.session.space == Some(layout) {
                    ui.label(RichText::new("(this session's)").color(c(th.text_dim)));
                }
            });
            let names: Vec<&str> = af27_space::speakers(layout)
                .iter()
                .map(|s| s.name)
                .collect();
            egui::Grid::new("route-speakers")
                .num_columns(2)
                .spacing(vec2(10.0, 4.0))
                .show(ui, |ui| {
                    for (k, name) in names.iter().enumerate() {
                        ui.label(*name);
                        let cur = m.speakers.get(*name).copied();
                        let by_place = format!("By place ({})", out_name(k));
                        let shown = match cur {
                            None => by_place.clone(),
                            Some(o) if usize::from(o) < width => out_name(usize::from(o)),
                            Some(o) => format!("{} (not on this device)", out_name(usize::from(o))),
                        };
                        let mut pick = cur;
                        egui::ComboBox::from_id_salt(format!("route-spk-{name}"))
                            .selected_text(shown)
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut pick, None, by_place);
                                for o in 0..width {
                                    ui.selectable_value(&mut pick, Some(o as u16), out_name(o));
                                }
                            });
                        if pick != cur {
                            match pick {
                                Some(o) => m.speakers.insert(name.to_string(), o),
                                None => m.speakers.remove(*name),
                            };
                        }
                        ui.end_row();
                    }
                });
            let lost = m.unplayable(Some(&names), width);
            if !lost.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "Not heard on this device ({width} outputs): {}.",
                        lost.join(", ")
                    ))
                    .color(c(th.accent_alt)),
                );
            }
            if ui.button("Every speaker by place").clicked() {
                m.speakers.clear();
            }
        });
    });
}

fn appearance(ui: &mut egui::Ui, d: &mut Draft, st: &State, th: &Theme) {
    group(ui, th, "Theme", |ui| {
        let shown = st
            .themes
            .iter()
            .find(|t| t.name == d.theme)
            .map_or(d.theme.clone(), |t| {
                if t.hidden {
                    "Secret Theme".into()
                } else {
                    t.name.clone()
                }
            });
        let before = d.theme.clone();
        egui::ComboBox::from_id_salt("settings-theme")
            .selected_text(shown)
            .width(240.0)
            .show_ui(ui, |ui| {
                for t in &st.themes {
                    if t.hidden && t.name != d.theme {
                        continue;
                    }
                    let label = if t.hidden { "Secret Theme" } else { &t.name };
                    ui.selectable_value(&mut d.theme, t.name.clone(), label);
                }
            });
        ui.horizontal(|ui| {
            if ui
                .button("Open Themes Folder")
                .on_hover_text("Put .af27theme files here to add them to the list")
                .clicked()
            {
                let dir = crate::theme::user_dir();
                let _ = std::fs::create_dir_all(&dir);
                crate::sessions::reveal_dir(&dir);
            }
            if ui.button("Reload Themes").clicked() {
                d.act = Some(PageAct::ReloadThemes);
            }
        });
        note(ui, th, "Your own themes: .af27theme files in the themes folder join this list. A file with the same name as a built-in theme replaces it.");
        if d.theme != before {
            // A new theme brings its own saved hues and looks.
            d.hues = crate::state::load_hues(&d.theme);
            if let Some(t) = st.themes.iter().find(|t| t.name == d.theme) {
                d.intro = t.intro;
                d.realistic = t.realistic;
            }
        }
    });
    let base = st
        .themes
        .iter()
        .find(|t| t.name == d.theme)
        .cloned()
        .unwrap_or_else(|| st.theme_base.clone());
    group(ui, th, "Theme colours", |ui| {
        note(
            ui,
            th,
            &format!("Turn the colours of {} to your own hues.", base.name),
        );
        let mut own = d.hues.is_some();
        if ui.checkbox(&mut own, "Use my own hues").changed() {
            d.hues = own.then(|| base.hues());
        }
        if let Some(h) = &mut d.hues {
            for (label, value, sat, val) in [
                ("Main light", &mut h.primary, 1.0, 1.0),
                ("Second light", &mut h.secondary, 0.82, 1.0),
                ("Base tint", &mut h.surface, 0.6, 0.35),
            ] {
                ui.horizontal(|ui| {
                    let sw = crate::theme::from_hsv(*value, sat, val);
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c(sw));
                    ui.add(
                        crate::widgets::Slider::new(value, 0.0..=359.0)
                            .suffix("°")
                            .text(label),
                    );
                });
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
                        *h = Hues {
                            primary: p,
                            secondary: s,
                            surface: u,
                        };
                    }
                }
            });
        }
    });
    group(ui, th, "Colour overrides", |ui| {
        note(
            ui,
            th,
            "Single colours of your own, laid over whichever theme is on.",
        );
        let fresh = match &d.hues {
            Some(h) => base.with_hues(h),
            None => base.clone(),
        };
        let o = &mut d.prefs.colors;
        for (label, slot, theirs) in [
            ("Waveform", &mut o.waveform, fresh.wave_peak),
            ("Playhead", &mut o.playhead, fresh.playhead),
            ("Accent", &mut o.accent, fresh.accent),
            ("Selection", &mut o.selection, fresh.selection),
        ] {
            ui.horizontal(|ui| {
                let mut on = slot.is_some();
                if ui.checkbox(&mut on, label).changed() {
                    *slot = on.then_some(theirs);
                }
                if let Some(col) = slot {
                    ui.color_edit_button_srgb(col);
                } else {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, c(theirs));
                    ui.label(RichText::new("the theme's").small().color(c(th.text_dim)));
                }
            });
        }
    });
    group(ui, th, "Visual effects", |ui| {
        for (m, label) in [
            (0u8, "Auto: every effect, cut back on its own if this device draws slowly"),
            (1, "Full: every effect, always"),
            (2, "Lite: no moving backdrop, glow or scan lines (lightest on slow or battery-powered devices)"),
        ] {
            ui.radio_value(&mut d.visuals, m, label);
        }
        if d.visuals == 0 && st.visuals_lite {
            note(ui, th, "Auto chose Lite for this device.");
        }
    });
    group(ui, th, "Start-up and surfaces", |ui| {
        ui.checkbox(
            &mut d.intro,
            "Play the boot sequence when AudiForm 2 starts",
        );
        if !base.columns.is_empty() {
            ui.checkbox(
                &mut d.realistic,
                "Draw the columns and tiles as real marble and glazed ceramic",
            );
        }
    });
}

// ------------------------------------------------------- shared drawing

/// A page-list entry: a thin accent bar and brighter text when chosen.
fn nav_item(ui: &mut egui::Ui, label: &str, on: bool, th: &Theme) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 25.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(r, 0.0, ca(th.accent, 16));
        p.rect_filled(
            Rect::from_min_size(r.min, vec2(2.5, r.height())),
            0.0,
            c(th.accent),
        );
    } else if resp.hovered() {
        p.rect_filled(r, 0.0, ca(th.accent, 8));
    }
    let col = if on || resp.hovered() {
        c(th.text)
    } else {
        c(th.text_dim)
    };
    p.text(
        pos2(r.left() + 14.0, r.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        col,
    );
    resp.clicked()
}

fn hairline(ui: &mut egui::Ui, th: &Theme) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, ca(th.border, 170));
}

fn group(ui: &mut egui::Ui, th: &Theme, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .size(10.5)
            .extra_letter_spacing(1.2)
            .color(c(th.text_dim)),
    );
    ui.add_space(2.0);
    ui.indent(title, add);
    ui.add_space(8.0);
}

fn note(ui: &mut egui::Ui, th: &Theme, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(11.0).color(c(th.text_dim))).wrap());
}

fn info(ui: &mut egui::Ui, th: &Theme, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label}:")).color(c(th.text_dim)));
        ui.label(value);
    });
}

// ------------------------------------------------------ keyboard shortcuts

#[derive(Default)]
pub struct ShortcutsUi {
    pub open: bool,
    search: String,
    /// Waiting for a key for (command, binding slot).
    pub capture: Option<(Cmd, usize)>,
}

impl ShortcutsUi {
    pub fn show(&mut self, ctx: &egui::Context, km: &mut Keymap, th: &Theme) {
        if !self.open {
            self.capture = None;
            return;
        }
        // Take the key for a binding being set before any widget sees it.
        if let Some((cmd, slot)) = self.capture {
            let got = ctx.input_mut(|i| {
                let mut got = None;
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if got.is_none() => {
                        got = Some(Binding::from_event(*key, *modifiers));
                        false
                    }
                    egui::Event::Key { .. } | egui::Event::Text(_) => false,
                    _ => true,
                });
                got
            });
            if let Some(b) = got {
                km.set(cmd, slot, b);
                km.save();
                self.capture = None;
            }
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .default_size(vec2(560.0, 560.0))
            .pivot(Align2::CENTER_CENTER)
            .default_pos(ctx.screen_rect().center())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search").color(c(th.text_dim)));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("a command or a key")
                            .desired_width(ui.available_width()),
                    );
                });
                note(ui, th, "Click a key to change it, then press the new combination. + adds another key; right-click a key to remove it.");
                ui.add_space(4.0);
                hairline(ui, th);
                let conflicts = km.conflicts();
                egui::TopBottomPanel::bottom("keys-foot")
                    .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 6.0)))
                    .show_separator_line(false)
                    .show_inside(ui, |ui| {
                        hairline(ui, th);
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Reset to Defaults").clicked() {
                                *km = Keymap::default();
                                self.capture = None;
                                changed = true;
                            }
                            if !conflicts.is_empty() {
                                ui.label(
                                    RichText::new(format!(
                                        "{} key{} used twice",
                                        conflicts.len(),
                                        if conflicts.len() == 1 { "" } else { "s" }
                                    ))
                                    .color(c(th.accent_alt)),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Close").clicked() {
                                        self.open = false;
                                    }
                                },
                            );
                        });
                    });
                let q = self.search.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut last_group = "";
                        for &cmd in Cmd::ALL {
                            let keys: Vec<Binding> = km.get(cmd).to_vec();
                            if !q.is_empty() {
                                let hay = format!(
                                    "{} {} {}",
                                    cmd.label(),
                                    cmd.group(),
                                    keys.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(" ")
                                )
                                .to_lowercase();
                                if !hay.contains(&q) {
                                    continue;
                                }
                            }
                            if cmd.group() != last_group {
                                last_group = cmd.group();
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(last_group.to_uppercase())
                                        .size(10.5)
                                        .extra_letter_spacing(1.2)
                                        .color(c(th.accent)),
                                );
                            }
                            changed |= self.row(ui, km, cmd, &keys, th);
                        }
                    });
            });
        if changed {
            km.save();
        }
        if !open {
            self.open = false;
        }
        if !self.open {
            self.capture = None;
        }
    }

    /// One command's row; true when its keys changed.
    fn row(
        &mut self,
        ui: &mut egui::Ui,
        km: &mut Keymap,
        cmd: Cmd,
        keys: &[Binding],
        th: &Theme,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_sized(
                vec2(190.0, 20.0),
                egui::Label::new(RichText::new(cmd.label()).color(c(th.text))).truncate(),
            );
            let mut remove = None;
            for (slot, b) in keys.iter().enumerate() {
                if self.capture == Some((cmd, slot)) {
                    waiting(ui, th);
                    continue;
                }
                let clash = km.clashes(cmd, *b);
                let col: Color32 = if clash.is_empty() {
                    c(th.text)
                } else {
                    c(th.accent_alt)
                };
                let mut r = ui
                    .add(egui::Button::new(
                        RichText::new(b.to_string()).monospace().color(col),
                    ))
                    .on_hover_text("Click to change; right-click to remove");
                if !clash.is_empty() {
                    let names: Vec<&str> = clash.iter().map(|c| c.label()).collect();
                    r = r.on_hover_text(format!("Also runs: {}", names.join(", ")));
                }
                if r.clicked() {
                    r.surrender_focus();
                    self.capture = Some((cmd, slot));
                }
                if r.secondary_clicked() {
                    remove = Some(slot);
                }
            }
            if self.capture == Some((cmd, keys.len())) {
                waiting(ui, th);
            } else if ui
                .small_button("+")
                .on_hover_text("Add another key")
                .clicked()
            {
                self.capture = Some((cmd, keys.len()));
            }
            if self.capture.is_some_and(|(c, _)| c == cmd) {
                if ui.small_button("Cancel").clicked() {
                    self.capture = None;
                }
            } else if keys != cmd.defaults().as_slice()
                && ui
                    .small_button("↺")
                    .on_hover_text("Back to this command's default keys")
                    .clicked()
            {
                km.reset(cmd);
                changed = true;
            }
            if let Some(s) = remove {
                km.remove(cmd, s);
                changed = true;
            }
        });
        changed
    }
}

fn waiting(ui: &mut egui::Ui, th: &Theme) {
    ui.add(
        egui::Button::new(RichText::new("Press keys...").color(c(th.background)))
            .fill(c(th.accent)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use af27_engine::{DeviceInfo, DeviceList, HostKind};

    fn listing(outs: &[&str]) -> DeviceList {
        DeviceList {
            outputs: outs
                .iter()
                .map(|n| DeviceInfo {
                    name: n.to_string(),
                    channels: 2,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_list_that_has_items_is_kept_when_the_driver_comes_back_empty() {
        let key: ScanKey = (HostKind::Asio, Some("SSL ASIO Driver".into()), None);
        let first = merge_scan(None, key.clone(), listing(&["SSL ASIO Driver"]));
        assert!(!first.stale);
        let mut empty = listing(&[]);
        empty.error = Some("busy".into());
        let again = merge_scan(Some(&first), key.clone(), empty.clone());
        assert!(again.stale);
        assert_eq!(again.list.outputs.len(), 1);
        assert_eq!(again.list.error.as_deref(), Some("busy"));
        // Another driver: its own (empty) list, not the old one.
        let other: ScanKey = (HostKind::Asio, Some("Focusrite USB ASIO".into()), None);
        let switched = merge_scan(Some(&first), other, empty);
        assert!(!switched.stale);
        assert!(switched.list.outputs.is_empty());
        // A fresh list with devices replaces the old one.
        let fresh = merge_scan(Some(&again), key, listing(&["A", "B"]));
        assert!(!fresh.stale);
        assert_eq!(fresh.list.outputs.len(), 2);
    }

    #[test]
    fn device_lists_follow_the_driver() {
        let mut a = AudioPrefs {
            output: Some("Speakers".into()),
            asio_driver: Some("SSL ASIO Driver".into()),
            ..Default::default()
        };
        // The system driver lists by output; the ASIO driver name is moot.
        assert_eq!(
            scan_key(&a),
            (HostKind::System, None, Some("Speakers".into()))
        );
        a.host = HostKind::Asio;
        assert_eq!(
            scan_key(&a),
            (HostKind::Asio, Some("SSL ASIO Driver".into()), None)
        );
    }

    #[test]
    fn old_audio_prefs_still_load() {
        let old = r#"{"output":"Speakers","input":null,"buffer":256}"#;
        let a: AudioPrefs = serde_json::from_str(old).unwrap();
        assert_eq!(a.host, HostKind::System);
        assert_eq!(a.buffer, Some(256));
        assert_eq!(a.map, af27_engine::OutputMap::default());
        let back: AudioPrefs = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        let c = a.choice();
        assert_eq!(c.output.as_deref(), Some("Speakers"));
        assert_eq!(c.rate, None);
    }
}
