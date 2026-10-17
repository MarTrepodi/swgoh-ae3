//! `ae3-gui`: the graphical front end. The window lives in the library
//! (`lib.rs`); this binary only starts it.

// Release builds on Windows are windowless GUI programs with no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    // The scripted smoke run CI starts; see `ae3_gui::smoke`.
    if let Some(report) = std::env::var_os(ae3_gui::smoke::ENV) {
        std::process::exit(ae3_gui::smoke::run(std::path::Path::new(&report)));
    }
    ae3_gui::run()
}
