#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod art;
mod xmb;
mod ytdl;
mod decode;
mod opus;
mod kit;
mod library;
mod loudness;
mod media;
mod player;
mod recommend;

fn main() -> eframe::Result<()> {
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()
        .map(|i| {
            let i = i.to_rgba8();
            egui::IconData {
                width: i.width(),
                height: i.height(),
                rgba: i.into_raw(),
            }
        });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Music Host")
        .with_app_id("com.musichost.app")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([900.0, 560.0])
        .with_drag_and_drop(true);
    // Screenshot/test runs must never steal clicks or focus from the user.
    let testing = std::env::var("MH_SCREENSHOT").is_ok();
    if testing {
        viewport = viewport.with_mouse_passthrough(true).with_active(false).with_inner_size([1280.0, 1300.0]);
    } else if std::env::var("MH_FPS").is_ok() {
        viewport = viewport.with_mouse_passthrough(true);
    }
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "Music Host",
        eframe::NativeOptions {
            viewport,
            // 4× MSAA: smooth edges on the animated ribbons and shapes.
            multisampling: 4,
            // Test screenshots use a fixed size instead of the saved window.
            persist_window: !testing,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
