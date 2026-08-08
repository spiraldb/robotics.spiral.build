//! `rerobot-studio`: the web UI's backend.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::Router;
use clap::Parser;
use rerobot::cameras;
use rerobot_studio::camera_manager::CameraManager;
use rerobot_studio::hardware::{
    ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS, HardwareHandle,
};
use rerobot_studio::routes::{self, AppState};
use rerobot_studio::ui;
use rerobot_studio::workspace::RobotWorkspace;
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How often the workspace directory is checked for outside changes.
const WORKSPACE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Set up, calibrate, and drive SO-ARM101 arms from a browser.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Set up, calibrate, and drive SO-ARM101 arms from a browser",
    long_about = "Serves the rerobot studio web UI and owns the hardware behind it. Each arm is one robot instance stored as one JSON file in the workspace directory, holding its port, cameras, motors, and calibration."
)]
struct Args {
    /// Directory holding one JSON file per robot instance.
    #[arg(long, default_value = "robots", value_name = "PATH")]
    workspace: PathBuf,

    /// Address the web UI is served on.
    #[arg(long, default_value = "127.0.0.1:8080", value_name = "ADDR:PORT")]
    bind: SocketAddr,

    /// Directory holding the compiled web UI.
    #[arg(long, value_name = "PATH")]
    ui_dir: Option<PathBuf>,

    /// Drive simulated motors instead of opening serial ports.
    #[arg(long)]
    simulate: bool,

    /// Skip camera discovery entirely.
    #[arg(long)]
    no_cameras: bool,

    /// Largest change, in encoder counts, applied to a joint per control cycle.
    #[arg(long, default_value_t = DEFAULT_MAX_STEP_COUNTS, value_name = "COUNTS")]
    max_step: i64,

    /// Largest per-cycle change used by the teleoperation follower.
    #[arg(long, default_value_t = DEFAULT_TELEOP_MAX_STEP_COUNTS, value_name = "COUNTS")]
    teleop_max_step: i64,
}

fn main() -> ExitCode {
    // Camera probing re-executes this binary as an isolated worker, so this
    // must come before argument parsing.
    match cameras::worker_mode() {
        Ok(Some(mode)) => {
            return match cameras::run_worker(&mode) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("error: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    }

    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("rerobot_studio=info,tower_http=warn")),
        )
        .init();

    if args.max_step <= 0 {
        eprintln!("error: --max-step must be greater than zero");
        return ExitCode::FAILURE;
    }
    if args.teleop_max_step <= 0 {
        eprintln!("error: --teleop-max-step must be greater than zero");
        return ExitCode::FAILURE;
    }

    // On macOS this puts up the system camera prompt, which has to happen on
    // the main thread before any runtime starts.
    let camera_access = if args.no_cameras {
        Ok(())
    } else {
        cameras::initialize_camera_access().map_err(|error| error.to_string())
    };
    if let Err(error) = &camera_access {
        tracing::warn!("camera access unavailable: {error}");
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: could not start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve(args, camera_access.err())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(args: Args, camera_access_error: Option<String>) -> Result<()> {
    let workspace = Arc::new(RobotWorkspace::open(args.workspace.clone())?);
    let hardware = Arc::new(HardwareHandle::spawn(ControlSettings {
        max_step_counts: args.max_step,
        teleop_max_step_counts: args.teleop_max_step,
        simulated: args.simulate,
        ..ControlSettings::default()
    }));
    let cameras = Arc::new(CameraManager::new());

    // Without permission a camera still enumerates and its stream still opens;
    // it just never delivers a frame. Say so instead of showing a blank box.
    if let Some(error) = camera_access_error {
        cameras.note_issue(format!("camera previews will stay blank: {error}"));
    }

    if !args.no_cameras {
        let discovery = cameras.clone();
        tokio::task::spawn_blocking(move || discovery.refresh_blocking()).await?;
    }

    // The workspace is shared with the command-line tools, so the studio has to
    // re-read it rather than assume it is the only writer.
    tokio::spawn({
        let workspace = workspace.clone();
        async move {
            let mut ticker = tokio::time::interval(WORKSPACE_POLL_INTERVAL);
            loop {
                ticker.tick().await;
                let workspace = workspace.clone();
                let reloaded =
                    tokio::task::spawn_blocking(move || workspace.reload_if_changed()).await;
                match reloaded {
                    Ok(Ok(true)) => tracing::info!("robot workspace changed on disk; reloaded"),
                    Ok(Ok(false)) => {}
                    Ok(Err(error)) => tracing::warn!("could not re-read the workspace: {error}"),
                    Err(error) => tracing::warn!("workspace reload task failed: {error}"),
                }
            }
        }
    });

    let ui_dir = ui::resolve_ui_dir(args.ui_dir.clone());
    let state = AppState {
        workspace: workspace.clone(),
        hardware: hardware.clone(),
        cameras: cameras.clone(),
        simulated: args.simulate,
    };
    let app = Router::new()
        .nest("/api", routes::api_router(state))
        .merge(ui::ui_router(ui_dir.as_deref()))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let address = listener.local_addr()?;

    tracing::info!("robot workspace: {}", workspace.directory().display());
    match &ui_dir {
        Some(directory) => tracing::info!("serving the web UI from {}", directory.display()),
        None => tracing::warn!(
            "the web UI has not been built; run `dx build --release` in crates/rerobot-ui"
        ),
    }
    if args.simulate {
        tracing::warn!("simulation mode: motors are emulated, no serial port is opened");
    }
    println!("rerobot studio is running at http://{address}");

    let shutdown = {
        let hardware = hardware.clone();
        let cameras = cameras.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
            // Release the cameras and stop the control loop before the process
            // exits, so devices are not left held open.
            cameras.shutdown();
            hardware.shutdown().await;
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
