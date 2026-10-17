//! `ae3-gui`: the graphical front end. The window lives in the library
//! (`lib.rs`); this binary only starts it.

// Release builds on Windows are windowless GUI programs with no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    ae3_gui::run()
}
