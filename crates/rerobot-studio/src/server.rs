//! `rerobot-studio`: the web UI's backend.

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::camera_manager::CameraManager;
use crate::episodes::EpisodeStore;
use crate::hardware::{
    ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS, HardwareHandle,
};
use crate::routes::{self, AppState};
use crate::workspace::RobotWorkspace;
use clap::Parser;
use rerobot::cameras;
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How often the workspace directory is checked for outside changes.
const WORKSPACE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// Browser event streams are intentionally long lived, so graceful draining
/// gets a short deadline instead of being allowed to hold process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Motor and camera cleanup normally completes within one control/frame cycle.
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1_500);

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

    /// Directory holding one file per recorded episode.
    #[arg(long, default_value = "episodes", value_name = "PATH")]
    episodes: PathBuf,

    /// Address the web UI is served on.
    #[arg(long, value_name = "ADDR:PORT")]
    bind: Option<SocketAddr>,

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

pub fn run() -> ExitCode {
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
    let Some(public) = dioxus_public_directory().filter(|path| path.is_dir()) else {
        eprintln!(
            "error: the Dioxus client bundle is missing; run `dx serve` for development or launch the server produced by `dx bundle`"
        );
        return ExitCode::FAILURE;
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("rerobot_studio=info,tower_http=warn")),
        )
        .init();
    tracing::debug!("Dioxus public directory: {}", public.display());

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
    // `dx serve` publishes its chosen address through the standard fullstack
    // environment. An explicit CLI value still wins for standalone launches.
    let bind = args
        .bind
        .unwrap_or_else(dioxus::cli_config::fullstack_address_or_localhost);
    let workspace = Arc::new(RobotWorkspace::open(args.workspace.clone())?);
    let episodes = Arc::new(EpisodeStore::open(args.episodes.clone())?);
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

    // The workspace and the episode directory are both shared with the
    // command-line tools, so the studio has to re-read them rather than assume
    // it is the only writer.
    tokio::spawn({
        let workspace = workspace.clone();
        let episodes = episodes.clone();
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

                let episodes = episodes.clone();
                let reloaded =
                    tokio::task::spawn_blocking(move || episodes.reload_if_changed()).await;
                match reloaded {
                    Ok(Ok(true)) => tracing::info!("episode directory changed on disk; reloaded"),
                    Ok(Ok(false)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!("could not re-read the episode directory: {error}")
                    }
                    Err(error) => tracing::warn!("episode reload task failed: {error}"),
                }
            }
        }
    });

    let state = AppState {
        workspace: workspace.clone(),
        episodes: episodes.clone(),
        hardware: hardware.clone(),
        cameras: cameras.clone(),
        simulated: args.simulate,
    };
    tokio::spawn(routes::auto_connect_saved_robots(state.clone()));
    let app = dioxus::server::router(crate::App)
        .nest("/api", routes::api_router(state))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;

    tracing::info!("robot workspace: {}", workspace.directory().display());
    tracing::info!("episode directory: {}", episodes.directory().display());
    if args.simulate {
        tracing::warn!("simulation mode: motors are emulated, no serial port is opened");
    }
    println!("rerobot studio is running at http://{address}");

    // Both the server and this task observe the same edge. The server begins
    // graceful draining immediately; this task enforces a deadline because
    // SSE connections are designed not to finish on their own.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let signal_task = tokio::spawn(shutdown_signals(shutdown_tx));
    let server_result = {
        let server = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_requested(shutdown_rx.clone()))
            .into_future();
        tokio::pin!(server);

        tokio::select! {
            result = &mut server => Some(result),
            () = shutdown_requested(shutdown_rx) => {
                match tokio::time::timeout(SERVER_DRAIN_TIMEOUT, &mut server).await {
                    Ok(result) => Some(result),
                    Err(_) => {
                        tracing::warn!("server drain timed out; closing remaining connections");
                        None
                    }
                }
            }
        }
    };
    tracing::info!("releasing motors and cameras");
    let camera_cleanup = tokio::task::spawn_blocking({
        let cameras = cameras.clone();
        move || cameras.shutdown()
    });
    let cleanup = async {
        let ((), camera_result) = tokio::join!(hardware.shutdown(), camera_cleanup);
        if let Err(error) = camera_result {
            tracing::warn!("camera cleanup task failed: {error}");
        }
    };
    if tokio::time::timeout(RESOURCE_SHUTDOWN_TIMEOUT, cleanup)
        .await
        .is_err()
    {
        tracing::warn!("resource cleanup timed out; the operating system will release devices");
    }
    signal_task.abort();
    if let Some(result) = server_result {
        result?;
    }
    Ok(())
}

/// Resolve only Dioxus's declared public-directory contract. This is a guard
/// around Dioxus 0.7's panicking missing-directory path, not bundle discovery.
fn dioxus_public_directory() -> Option<PathBuf> {
    std::env::var_os("DIOXUS_PUBLIC_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()?
                .parent()
                .map(|path| path.join("public"))
        })
}

/// Notify every shutdown waiter on the first Ctrl+C and make the second press
/// an unconditional escape hatch. Once Tokio installs its signal handler the
/// operating system's default second-press behavior no longer applies, so it
/// has to be restored explicitly.
async fn shutdown_signals(shutdown: tokio::sync::watch::Sender<bool>) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!("could not listen for Ctrl+C: {error}");
    }
    eprintln!("shutdown requested; press Ctrl+C again to force exit");
    let _ = shutdown.send(true);

    if tokio::signal::ctrl_c().await.is_ok() {
        eprintln!("forcing exit");
        std::process::exit(130);
    }
}

async fn shutdown_requested(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_waiters_release_when_the_signal_is_published() {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(shutdown_requested(receiver));

        sender.send(true).expect("the waiter is listening");
        tokio::time::timeout(std::time::Duration::from_millis(100), waiter)
            .await
            .expect("the waiter must not hang")
            .expect("the waiter task must finish");
    }

    #[tokio::test]
    async fn an_already_published_shutdown_does_not_wait_for_another_change() {
        let (_sender, receiver) = tokio::sync::watch::channel(true);
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            shutdown_requested(receiver),
        )
        .await
        .expect("an existing shutdown request is immediately visible");
    }
}
