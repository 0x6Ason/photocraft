//! Photocraft desktop app.
//!
//! Usage: `photocraft [--control <port>] [files…]`
//!
//! `--control <port>` (or `PHOTOCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control server.
//! Each line `{"id":1,"method":"ui.inspect","params":{}}` gets a reply line
//! `{"id":1,"ok":true,"result":…}`. See `photocraft_ui_egui::control` for the methods.

// Release builds on Windows are GUI-subsystem apps, so launching from the Start Menu or Explorer
// doesn't open a console window. (`--version` output then only shows when redirected.)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod control_server;
mod services;

use photocraft_engine::Session;
use photocraft_ui_egui::PhotocraftApp;

fn main() -> eframe::Result {
    let mut control_port: Option<u16> = std::env::var("PHOTOCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok());
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--control" => control_port = args.next().and_then(|p| p.parse().ok()),
            "--version" => {
                println!("photocraft {}", photocraft_engine::build_info::long_version());
                return Ok(());
            }
            // Old macOS passes a process serial number when launched from Finder.
            _ if a.starts_with("-psn_") => {}
            _ => files.push(a),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("PhotoCraft").with_inner_size([1440.0, 900.0]).with_min_inner_size([760.0, 480.0]).with_drag_and_drop(true).with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false),
        ..Default::default()
    };
    eframe::run_native(
        "Photocraft",
        options,
        Box::new(move |cc| {
            let mut app = PhotocraftApp::new(Session::new(), services::native());
            app.integrated_titlebar = cfg!(target_os = "macos");
            // Preferences › Performance › Use Graphics Processor.
            if let Some(rs) = cc.wgpu_render_state.clone()
                && std::env::var_os("PHOTOCRAFT_CPU_CANVAS").is_none()
                && app.session.prefs().performance.use_gpu
            {
                app.set_wgpu(rs);
            }
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            for f in files {
                match std::fs::read(&f) {
                    Ok(bytes) => {
                        let name = std::path::Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(f.clone());
                        if let Err(e) = app.open_bytes(&name, &bytes) {
                            eprintln!("photocraft: {f}: {e}");
                        } else if let Some(st) = app.session.active_mut() {
                            st.path = Some(f.clone());
                        }
                    }
                    Err(e) => eprintln!("photocraft: {f}: {e}"),
                }
            }
            Ok(Box::new(app))
        }),
    )
}
