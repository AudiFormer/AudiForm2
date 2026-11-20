// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform27 contributors.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_icon;
mod caustics;
mod classic_themes;
mod clip_props;
mod corners;
mod export;
mod files_panel;
mod fx;
mod fx_menu;
mod hud;
mod hwcursor;
mod inspector;
mod keymap;
mod lane_menu;
mod midi_in;
mod mixing;
mod multitrack;
mod new_dialogs;
mod overview;
mod panels;
mod phasewheel;
mod pianoroll;
mod pictures;
mod plugins;
mod pointers;
mod rack;
mod record_input;
mod retro;
mod ruler;
mod selfshot;
mod separation;
mod sessions;
mod settings;
mod skin;
mod space;
mod space_export;
mod spectral_select;
mod splash;
mod state;
mod tempo;
mod theme;
mod track_views;
mod transport;
mod ui;
mod visualizer;
mod waveform;
mod widgets;
mod workspace;
mod zoom;
mod zoom_bench;

fn main() -> eframe::Result<()> {
    // Started again as a VST3 plug-in host or scanner (see af27-vst3).
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(code) = af27_vst3::main_with_args(&args) {
        std::process::exit(code);
    }
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
    let mut viewport = egui::ViewportBuilder::default().with_title("AudiForm 2");
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
        ..Default::default()
    };
    eframe::run_native(
        "Audiform27",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, files)))),
    )
}
