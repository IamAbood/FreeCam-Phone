#![windows_subsystem = "windows"]
//! FreeCam Phone receiver: finds the phone, decodes its stream and feeds the
//! virtual camera / microphone outputs.

mod adb;
mod audio;
mod autostart;
mod conn;
mod decode;
mod settings;
mod ui;
mod vcam;

use eframe::egui;

fn main() -> eframe::Result {
    let shared = conn::Shared::new(settings::Settings::load());
    conn::spawn(shared.clone());
    let minimized = std::env::args().any(|a| a == "--minimized");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FreeCam Phone")
            .with_inner_size([1100.0, 680.0])
            .with_visible(!minimized)
            .with_icon(egui::IconData {
                rgba: ui::icon_rgba(64),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "FreeCam Phone",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, shared)))),
    )
}
