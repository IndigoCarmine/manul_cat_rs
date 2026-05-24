#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kuromame_rs::app::KuromameApp;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions::default();

    eframe::run_native(
        "Kuromame - Molecule Editor",
        options,
        Box::new(|cc| Ok(Box::new(KuromameApp::new(cc)))),
    )
}
