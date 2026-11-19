// SPDX-License-Identifier: LicenseRef-Audiform27
// Copyright (c) The Audiform27 contributors.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod classic_themes;
mod fx;
mod hud;
mod inspector;
mod multitrack;
mod panels;
mod phasewheel;
mod pianoroll;
mod pictures;
mod separation;
mod skin;
mod space;
mod splash;
mod state;
mod theme;
mod ui;
mod visualizer;
mod waveform;
mod workspace;
mod zoom;

fn main() -> eframe::Result<()> {
    fx::register();
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let glow_first = std::env::var("AF2_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("glow"));
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AudiForm 2")
            .with_inner_size([1600.0, 940.0])
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
