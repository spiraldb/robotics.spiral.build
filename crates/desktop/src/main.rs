//! Spiral Robotics Studio as a desktop app. The Studio backend runs inside
//! this process on a loopback port, and one window shows its web interface.

use std::process::ExitCode;
use std::sync::Mutex;

use spiral_robotics_backend::{DesktopFiles, DesktopStudio};
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

const MAIN_WINDOW: &str = "main";

/// The Studio this app started, until the app exits and stops it.
struct RunningStudio(Mutex<Option<DesktopStudio>>);

fn main() -> ExitCode {
    match DesktopStudio::run_camera_worker() {
        Ok(true) => return ExitCode::SUCCESS,
        Ok(false) => {}
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    }
    let app = tauri::Builder::default()
        // A second launch shows this window instead of starting another
        // Studio on the same database and robots.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let files = DesktopFiles::new(
                app.path().resource_dir()?.join("public"),
                app.path().app_data_dir()?.join("studio.sqlite3"),
            );
            let studio = DesktopStudio::start(files)?;
            let url = format!("http://{}/", studio.address()).parse()?;
            app.manage(RunningStudio(Mutex::new(Some(studio))));
            WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::External(url))
                .title("Spiral Robotics Studio")
                .inner_size(1440.0, 900.0)
                .min_inner_size(1024.0, 640.0)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.run(|app, event| {
        if let RunEvent::Exit = event
            && let Some(running) = app.try_state::<RunningStudio>()
            && let Some(studio) = running.0.lock().ok().and_then(|mut studio| studio.take())
        {
            studio.stop();
        }
    });
    ExitCode::SUCCESS
}
