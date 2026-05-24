#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use manul_cat_rs::app::KuromameApp;
use std::path::Path;

fn load_icon() -> Option<eframe::egui::IconData> {
    let icon_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/Manuru.ico");
    let icon_bytes = std::fs::read(icon_path).ok()?;
    let icon = image::load_from_memory_with_format(&icon_bytes, image::ImageFormat::Ico).ok()?;
    let rgba = icon.into_rgba8();
    let (width, height) = rgba.dimensions();

    Some(eframe::egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Manul")
            .with_icon(load_icon().unwrap_or_else(|| eframe::egui::IconData {
                rgba: vec![0, 0, 0, 0],
                width: 1,
                height: 1,
            })),
        ..eframe::NativeOptions::default()
    };

    eframe::run_native(
        "Manul",
        options,
        Box::new(|cc| Ok(Box::new(KuromameApp::new(cc)))),
    )
}
