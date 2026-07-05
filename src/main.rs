#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use manul_cat_rs::app::KuromameApp;
use std::path::PathBuf;

/// Window/taskbar icon, embedded at compile time so it is available no matter
/// where the executable is installed (the source tree is not present on an
/// end-user's machine).
const ICON_BYTES: &[u8] = include_bytes!("../resources/Manuru.ico");

fn load_icon() -> Option<eframe::egui::IconData> {
    let icon = image::load_from_memory_with_format(ICON_BYTES, image::ImageFormat::Ico).ok()?;
    let rgba = icon.into_rgba8();
    let (width, height) = rgba.dimensions();

    Some(eframe::egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

fn main() -> Result<(), eframe::Error> {
    // Files passed on the command line (e.g. via a Windows file association or
    // "Open with"). Loaded into the app once it is constructed.
    let startup_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Manul - A Molecular Viewer for GROMACS")
            .with_icon(load_icon().unwrap_or_else(|| eframe::egui::IconData {
                rgba: vec![0, 0, 0, 0],
                width: 1,
                height: 1,
            })),
        ..eframe::NativeOptions::default()
    };

    eframe::run_native(
        "Manul - A Molecular Viewer for GROMACS",
        options,
        Box::new(move |cc| {
            let mut app = KuromameApp::new(cc);
            app.load_paths(startup_paths);
            Ok(Box::new(app))
        }),
    )
}
