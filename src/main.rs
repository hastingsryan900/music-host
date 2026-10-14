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
mod lyrics;
mod media;
mod menubar;
mod player;
mod recommend;
mod stream;
mod update;

// Pulls in the libopus that audiopus_sys builds (see opus.rs).
#[cfg(windows)]
use audiopus_sys as _;

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
    install_crash_report();
    // 4× MSAA gives smooth edges on the animated ribbons and shapes, but some
    // graphics drivers (VMs, older GPUs) can't make such a window: fall back
    // to plain OpenGL rather than failing to open.
    let mut last = None;
    for msaa in [4, 0] {
        log(&format!("starting (msaa {msaa})"));
        let result = eframe::run_native(
            "Music Host",
            eframe::NativeOptions {
                viewport: viewport.clone(),
                multisampling: msaa,
                // Test screenshots use a fixed size instead of the saved window.
                persist_window: !testing,
                ..Default::default()
            },
            Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
        );
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                log(&format!("couldn't open the window: {e}"));
                last = Some(e);
            }
        }
    }
    let e = last.expect("tried at least once");
    fatal(&format!("Music Host couldn't open its window.\n\n{e}\n\nYour graphics driver may not support OpenGL 3.3. Updating it usually fixes this."));
    Err(e)
}

/// Append a line to the app's log (in the data folder), for diagnosing
/// problems on machines where there's no console.
fn log(line: &str) {
    use std::io::Write;
    let path = library::app_dir().join("log.txt");
    // Keep it small.
    if std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Show an error the user can actually see (release builds have no console).
fn fatal(msg: &str) {
    log(msg);
    let _ = rfd::MessageDialog::new()
        .set_title("Music Host")
        .set_description(msg)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn install_crash_report() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!(
            "Music Host hit a problem and has to close.\n\n{info}\n\nDetails are saved in {}",
            library::app_dir().join("log.txt").display()
        );
        fatal(&msg);
    }));
}
