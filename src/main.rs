// Sin consola en las builds de distribución.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod audio;
mod creator;
mod export;
mod gpu;
mod tutorial;

use app::NullframeApp;
use eframe::egui;

fn window_icon() -> Option<egui::IconData> {
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?;
    let rgba = icon.to_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("nullframe")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([760.0, 540.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "nullframe",
        options,
        Box::new(|creation_context| Ok(Box::new(NullframeApp::new(creation_context)))),
    )
}
