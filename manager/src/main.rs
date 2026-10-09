#![windows_subsystem = "windows"]
//! Manager binary. Contains no webview code: sign-in runs in ram-login.exe (see auth::login).
mod app;
mod auth;
mod launcher;
mod runtime;
mod storage;
mod ui;
mod update;
mod watcher;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Roblox Account Manager v{}", update::VERSION))
            .with_inner_size([1080.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Roblox Account Manager",
        opts,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
