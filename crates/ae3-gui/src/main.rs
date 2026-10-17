//! `ae3-gui`: the graphical front end. Like the CLI it only presents what
//! `ae3-core` does (ADR-0001).

// Release builds on Windows are windowless GUI programs with no console.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AE3")
            .with_inner_size([1024.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native("AE3", options, Box::new(|_cc| Ok(Box::new(Ae3App))))
}

struct Ae3App;

impl eframe::App for Ae3App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |_ui| {});
    }
}
