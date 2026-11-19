// SPDX-License-Identifier: LicenseRef-Audiform27
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
    /// Space while playing: stop and go back to where play started (true)
    /// or pause where it is (false). Mirrors `State::return_on_stop`.
    pub space_returns: bool,
    /// Scroll the view along with the playhead while playing.
    pub follow_playhead: bool,
    pub autosave: bool,
    pub autosave_minutes: u32,
    /// Bring the Effects panel up when an effect is picked from the menu.
    pub show_effects_on_pick: bool,
    pub colors: ColorOverrides,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            renderer: Renderer::Wgpu,
            space_returns: true,
            follow_playhead: true,
            autosave: true,
            autosave_minutes: 2,
            show_effects_on_pick: true,
            colors: ColorOverrides::default(),
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
        "Audio Devices" => &["Choosing the output device for playback, previews and monitoring (falling back to the system default when it is unplugged)."],
        "Editing" => &["The shape new fades start as: straight, or a curve smooth at both ends."],
        "Multitrack" => &["Exclusive solo: soloing one track un-solos the others, in one undoable step."],
        "Clips" => &["Clip auto-crossfade where clips overlap."],
        "Playback and Recording" => &[
            "Meter yellow and red levels.",
            "Recording latency offset, punch-in and pre-roll.",
        ],
        "Time Display" => &["The time format (decimal, samples, bars and beats, SMPTE)."],
        "Loudness" => &["The loudness region and target."],
        "Cache and Files" => &["Cache sizes, and clearing the waveform drawings (they are rebuilt on demand)."],
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
            }
            "Effects" => self.prefs.show_effects_on_pick = d.show_effects_on_pick,
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
        if self.prefs != st.prefs {
            st.prefs = self.prefs.clone();
            st.prefs.save();
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
                    (WheelAnchor::Pointer, "The mouse pointer"),
                    (
                        WheelAnchor::BetweenPointerAndMiddle,
                        "Between the pointer and the middle of the view (drifts less)",
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
        "Audio Devices" => {
            group(ui, th, "Output", |ui| {
                match (&st.engine, &st.engine_error) {
                    (Some(e), _) => {
                        info(ui, th, "Device", &e.device);
                        info(ui, th, "Sample rate", &format!("{} Hz", e.rate));
                        note(ui, th, "Playback, previews and monitoring use the system's default output device.");
                    }
                    (None, Some(err)) => {
                        ui.label(RichText::new(err).color(c(th.accent_alt)));
                    }
                    _ => note(ui, th, "No audio output is open."),
                }
            });
        }
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
                let p = data_dir().join("autosave").join("autosave.audfrm");
                note(
                    ui,
                    th,
                    &format!(
                        "The copy goes to {}; your own file is never touched.",
                        p.display()
                    ),
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
            group(ui, th, "The space bar while playing", |ui| {
                ui.radio_value(
                    &mut d.prefs.space_returns,
                    true,
                    "Stops and goes back to where play started",
                );
                ui.radio_value(&mut d.prefs.space_returns, false, "Pauses where it is");
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
                        egui::Slider::new(value, 0.0..=359.0)
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
