// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) The Audiform27 contributors.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod multitrack;
mod panels;
mod state;
mod theme;
mod ui;
mod waveform;

fn main() -> eframe::Result<()> {
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Audiform27")
            .with_inner_size([1600.0, 940.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Audiform27",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, files)))),
    )
}
