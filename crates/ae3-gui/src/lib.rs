//! `ae3-gui`: the graphical front end. Like the CLI it only presents what
//! `ae3-core` does (ADR-0001).
//!
//! The window is a library so that `ae3-gui-tests` can drive it headless:
//! [`Ae3App::frame`] runs one frame of the real [`ui::show`], and
//! [`Ae3App::with_services`] reaches the network through a transport of the
//! caller's choosing. The binary in `main.rs` only calls [`run`] (or
//! [`smoke::run`] for the scripted smoke run).

// Pixel sizes (at most 16384) and counts shown as progress or layout are far
// below the 2^24 that f32 represents exactly.
#![allow(
    clippy::cast_precision_loss,
    reason = "UI geometry and progress fractions from small integers"
)]

pub mod app;
mod highlight;
pub mod model;
pub mod output;
pub mod smoke;
pub mod tasks;
pub mod ui;
pub mod worker;

use std::path::PathBuf;

pub use app::Ae3App;

use ae3_core::config::{Config, ConfigFile, Settings};
use eframe::egui;

/// The settings the window starts with, as `Config::load` gives them.
pub struct Startup {
    pub config: Config,
    pub file: ConfigFile,
    /// Where the settings are saved; `None` when there is no such place.
    pub path: Option<PathBuf>,
    /// Why the settings could not be loaded, shown in the window.
    pub notice: Option<String>,
}

impl Startup {
    /// AE3_* environment variables over the file over the defaults. A
    /// broken file or variable is reported and defaults used.
    #[must_use]
    pub fn load() -> Self {
        match Config::load(&Settings::default()) {
            Ok(loaded) => Self {
                config: loaded.config,
                file: loaded.file,
                path: Some(loaded.path),
                notice: None,
            },
            Err(e) => Self {
                config: Config::default(),
                file: ConfigFile::default(),
                path: ConfigFile::path().ok(),
                notice: Some(format!("Settings not loaded: {e}")),
            },
        }
    }
}

/// The window: title, size and the depth buffer the Mesh view needs.
#[must_use]
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
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
    }
}

/// Opens the window and runs it until it is closed.
pub fn run() -> eframe::Result {
    let Startup {
        config,
        file,
        path,
        notice,
    } = Startup::load();
    eframe::run_native(
        "AE3",
        native_options(),
        Box::new(move |cc| {
            let mut app = Ae3App::new(&cc.egui_ctx, config, file, path, notice);
            app.start();
            Ok(Box::new(app))
        }),
    )
}
