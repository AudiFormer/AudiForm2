// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod af1_import;
mod app_icon;
mod archive;
mod linked;
mod caustics;
mod chapters;
mod classic_themes;
mod clip_ops;
mod clip_lanes;
mod clip_props;
mod corners;
mod desktop_entry;
mod diagnostics;
mod dir_cache;
mod eq_editor;
mod export;
mod file_dialog;
mod files_panel;
mod fx;
mod fx_menu;
mod fx_windows;
mod generate_ui;
mod graph;
mod hud;
mod hwcursor;
mod inspector;
mod instruments;
mod keymap;
mod lane_menu;
mod linux_desktop;
mod loudness_meter;
mod memstat;
mod meters;
mod midi_in;
mod midi_lanes;
mod mixing;
mod multitrack;
mod new_dialogs;
mod noise_print;
mod spatialize_ui;
mod overlap;
mod overview;
mod panels;
mod phasewheel;
mod pianoroll;
mod pictures;
mod plugins;
mod podcast_tools;
mod pointers;
mod prerender;
mod rack;
mod rack_builtins;
mod record_input;
mod retro;
mod ruler;
mod selfshot;
mod separation;
mod sessions;
mod settings;
mod skin;
mod system_look;
mod space;
mod space_export;
mod spectral_select;
mod splash;
mod backlit;
mod synth_ui;
mod state;
mod tempo;
mod theme;
mod track_views;
mod transport;
mod tube_master_ui;
mod ui;
mod updater;
mod video;
mod visualizer;
mod wave_channels;
mod waveform;
mod wayland_dnd;
mod widgets;
mod workspace;
mod zoom;
mod zoom_bench;

fn main() -> eframe::Result<()> {
    // Started again as a VST3 plug-in host or scanner (see af2-vst3).
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(code) = af2_vst3::main_with_args(&args) {
        af2_vst3::exit_now(code);
    }
    // Settings from before 2.0.4 move from <data>/Audiform2 to
    // <data>/Audiform2 before anything reads or writes them.
    let _ = state::data_dir();
    fx::register();
    plugins::init();
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    // AF2_RENDERER, when set, wins over the choice in Preferences.
    let glow_first = match std::env::var("AF2_RENDERER") {
        Ok(v) => v.eq_ignore_ascii_case("glow"),
        Err(_) => settings::Prefs::load().renderer == settings::Renderer::Glow,
    };
    let first = if glow_first {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    };
    match run(first, files.clone()) {
        // A machine whose graphics driver has no DirectX 12 / Vulkan (old
        // or virtual machines) still gets a window, through OpenGL.
        Err(e) if !glow_first => {
            eprintln!("wgpu could not start ({e}); using OpenGL");
            run(eframe::Renderer::Glow, files)
        }
        r => r,
    }
}

fn run(renderer: eframe::Renderer, files: Vec<std::path::PathBuf>) -> eframe::Result<()> {
    // The app ID matches the desktop entry, so Linux desktops show its icon.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("AudiForm 2")
        .with_app_id("io.github.audiformer.AudiForm2");
    // The Zero Point icon until the theme's own is put up (app_icon.rs).
    if let Some(icon) = app_icon::default_icon_data() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_inner_size(selfshot::window_size().unwrap_or([1600.0, 940.0]))
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true),
        // wgpu (DirectX 12, Vulkan or Metal) presents reliably on more
        // machines than OpenGL; AF2_RENDERER=glow switches back.
        renderer,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            desired_maximum_frame_latency: Some(1),
            // Low-latency drawing: present at once instead of waiting for
            // the refresh (the app caps the frame rate itself, ui.rs).
            present_mode: if settings::Prefs::load().low_latency {
                eframe::wgpu::PresentMode::AutoNoVsync
            } else {
                eframe::wgpu::PresentMode::AutoVsync
            },
            ..Default::default()
        },
        vsync: !settings::Prefs::load().low_latency,
        ..Default::default()
    };
    eframe::run_native(
        "Audiform2",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, files)))),
    )
}
