//! `ae3-gui`: the graphical front end. Like the CLI it only presents what
//! `ae3-core` does (ADR-0001).

// Release builds on Windows are windowless GUI programs with no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Pixel sizes (at most 16384) and counts shown as progress or layout are far
// below the 2^24 that f32 represents exactly.
#![allow(
    clippy::cast_precision_loss,
    reason = "UI geometry and progress fractions from small integers"
)]

mod app;
mod highlight;
mod model;
mod output;
mod tasks;
mod ui;
mod worker;

use ae3_core::config::{Config, ConfigFile, Settings};
use eframe::egui;

fn main() -> eframe::Result {
    // Settings: AE3_* environment variables over the file over the
    // defaults. A broken file or variable is reported and defaults used.
    let (config, file, path, notice) = match Config::load(&Settings::default()) {
        Ok(loaded) => (loaded.config, loaded.file, Some(loaded.path), None),
        Err(e) => (
            Config::default(),
            ConfigFile::default(),
            ConfigFile::path().ok(),
            Some(format!("Settings not loaded: {e}")),
        ),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AE3")
            // Wayland app_id and X11 WM_CLASS; matches StartupWMClass in
            // packaging/linux/ae3-gui.desktop so docks group the window.
            .with_app_id("ae3-gui")
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([720.0, 480.0]),
        // The 3D Mesh view depth-tests against the window's framebuffer.
        depth_buffer: 24,
        ..Default::default()
    };
    eframe::run_native(
        "AE3",
        options,
        Box::new(move |cc| {
            let mut app = app::Ae3App::new(&cc.egui_ctx, config, file, path, notice);
            app.start();
            Ok(Box::new(app))
        }),
    )
}
