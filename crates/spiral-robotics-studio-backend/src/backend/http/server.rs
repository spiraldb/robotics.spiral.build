//! `spiral-robotics-studio`: the web UI's backend.

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::backend::http::routes::{self, AppState, BootstrapState};
use crate::backend::recording::episodes::EpisodeStore;
use crate::backend::robotics::cameras::CameraManager;
use crate::backend::robotics::video::VideoEncoderSettings;
use crate::backend::robots::workspace::RobotWorkspace;
use crate::backend::rollout::manager::RolloutManager;
use crate::backend::storage::manager::StorageManager;
use crate::backend::storage::repository::SpiralRepository;
use crate::backend::storage::settings::{
    BackendStateStore, StudioPreferencesStore, environment_spiral_endpoint,
    environment_spiral_spfs_endpoint, normalize_spiral_endpoint, normalize_spiral_spfs_endpoint,
};
use crate::backend::training::manager::TrainingManager;
use crate::backend::training::remote::ConfiguredHttpTrainingBackend;
use crate::backend::workflow::agent::WorkflowAgentManager;
use crate::backend::workflow::manager::WorkflowManager;
use clap::Parser;
use spiral_robotics_hardware::cameras;
use spiral_robotics_hardware::{
    ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS, HardwareHandle,
};
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Browser event streams are intentionally long lived, so graceful draining
/// gets a short deadline instead of being allowed to hold process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Motor and camera cleanup normally completes within one control/frame cycle;
/// a final MP4 may need a few more seconds to flush its index and fast-start
/// metadata without corrupting the last recorded episode.
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// Set up, calibrate, and drive SO-ARM101 arms from a browser.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Set up, calibrate, and drive SO-ARM101 arms from a browser",
    long_about = "Serves the Spiral Robotics Studio web UI, owns the hardware behind it, and stores all durable Studio data directly in SpiralDB."
)]
struct Args {
    /// Address the web UI is served on.
    #[arg(long, value_name = "ADDR:PORT")]
    bind: Option<SocketAddr>,

    /// Permit the unauthenticated Studio control API to listen beyond loopback.
    #[arg(long, requires = "bind")]
    allow_remote_access: bool,

    /// Skip camera discovery entirely.
    #[arg(long)]
    no_cameras: bool,

    /// FFmpeg executable used by the backend for episode camera recording.
    #[arg(long, default_value = "ffmpeg", value_name = "PATH")]
    ffmpeg: String,

    /// FFmpeg video codec used for browser-playable episode MP4s.
    #[arg(long, default_value = "libx264", value_name = "CODEC")]
    video_codec: String,

    /// Optional initial Spiral project. Settings and the backend state file are
    /// used when this and SPIRAL_PROJECT_ID are omitted.
    #[arg(long, value_name = "PROJECT")]
    spiral_project: Option<String>,

    /// Namespace containing every Studio-owned relation.
    #[arg(long, default_value = "spiral_robotics", value_name = "NAMESPACE")]
    spiral_namespace: String,

    /// Largest change, in encoder counts, applied to a joint per control cycle.
    #[arg(long, default_value_t = DEFAULT_MAX_STEP_COUNTS, value_name = "COUNTS")]
    max_step: i64,

    /// Largest per-cycle change used by the teleoperation follower.
    #[arg(long, default_value_t = DEFAULT_TELEOP_MAX_STEP_COUNTS, value_name = "COUNTS")]
    teleop_max_step: i64,
}

pub fn run(app: fn() -> dioxus::prelude::Element) -> ExitCode {
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
                .unwrap_or_else(|_| EnvFilter::new("spiral_robotics_studio=info,tower_http=warn")),
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

    let backend_state = match BackendStateStore::open_default() {
        Ok(state) => Arc::new(state),
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let spiral_endpoint = match normalize_spiral_endpoint(
        &backend_state
            .spiral_endpoint()
            .unwrap_or_else(environment_spiral_endpoint),
    ) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let spiral_spfs_endpoint = match normalize_spiral_spfs_endpoint(
        &backend_state
            .spiral_spfs_endpoint()
            .unwrap_or_else(environment_spiral_spfs_endpoint),
    ) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
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

    match runtime.block_on(serve(
        args,
        camera_access.err(),
        backend_state,
        spiral_endpoint,
        spiral_spfs_endpoint,
        app,
    )) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(
    args: Args,
    camera_access_error: Option<String>,
    backend_state: Arc<BackendStateStore>,
    spiral_endpoint: String,
    spiral_spfs_endpoint: String,
    app: fn() -> dioxus::prelude::Element,
) -> Result<()> {
    // `dx serve` publishes its chosen address through the standard fullstack
    // environment. An explicit CLI value still wins for standalone launches.
    let bind = args
        .bind
        .unwrap_or_else(dioxus::cli_config::fullstack_address_or_localhost);
    validate_bind_address(bind, args.allow_remote_access)?;
    let spiral_project = args
        .spiral_project
        .clone()
        .or_else(|| std::env::var("SPIRAL_PROJECT_ID").ok())
        .filter(|project| !project.trim().is_empty())
        .or_else(|| backend_state.project_id());
    if !spiral_robotics_contracts::spiral::is_identifier(args.spiral_namespace.trim()) {
        return Err(std::io::Error::other(
            "Spiral namespace must start with a letter or '_' and contain only letters, numbers, or '_'",
        )
        .into());
    }
    let namespace = args.spiral_namespace.trim().to_owned();
    let repository = Arc::new(
        SpiralRepository::new(
            spiral_endpoint,
            spiral_spfs_endpoint,
            spiral_project,
            backend_state.credentials(),
            namespace,
        )
        .map_err(std::io::Error::other)?,
    );
    let workspace = Arc::new(RobotWorkspace::new(repository.clone()));
    let episodes = Arc::new(EpisodeStore::new(repository.clone()));
    let storage = Arc::new(StorageManager::new(repository));
    let preferences = Arc::new(StudioPreferencesStore::new(Default::default()));
    let training = Arc::new(TrainingManager::new(
        storage.clone(),
        Arc::new(ConfiguredHttpTrainingBackend::new(preferences.clone())),
    ));
    let workflow = Arc::new(WorkflowManager::new(storage.clone()));
    let catalog_lease = Arc::new(
        spiral_robotics_studio_application::catalog_lease::CatalogMutationLease::default(),
    );
    let workflow_agent = Arc::new(WorkflowAgentManager::new(
        storage.clone(),
        workflow.clone(),
        catalog_lease.clone(),
    ));
    let hardware = Arc::new(HardwareHandle::spawn(ControlSettings {
        max_step_counts: args.max_step,
        teleop_max_step_counts: args.teleop_max_step,
        ..ControlSettings::default()
    }));
    let cameras = Arc::new(CameraManager::new());
    let rollout = RolloutManager::new(
        storage.clone(),
        hardware.clone(),
        cameras.clone(),
        workspace.clone(),
        preferences.clone(),
    )
    .map_err(std::io::Error::other)?;

    // Without permission a camera still enumerates and its stream still opens;
    // it just never delivers a frame. Say so instead of showing a blank box.
    if let Some(error) = camera_access_error {
        cameras.note_issue(format!("camera previews will stay blank: {error}"));
    }

    let state = AppState {
        workspace: workspace.clone(),
        episodes: episodes.clone(),
        hardware: hardware.clone(),
        cameras: cameras.clone(),
        backend_state,
        preferences,
        storage,
        training: training.clone(),
        workflow: workflow.clone(),
        workflow_agent: workflow_agent.clone(),
        rollout: rollout.clone(),
        bootstrap: BootstrapState::new(),
        catalog_lease,
        recording_video: Arc::new(tokio::sync::Mutex::new(None)),
        video_encoder: VideoEncoderSettings {
            executable: args.ffmpeg.clone(),
            codec: args.video_codec.clone(),
        },
    };
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;

    let public_url =
        std::env::var("SPIRAL_ROBOTICS_PUBLIC_URL").unwrap_or_else(|_| format!("http://{address}"));
    println!("Spiral Robotics Studio is running at {public_url}");

    let bootstrap = tokio::spawn(bootstrap_services(state.clone(), !args.no_cameras));
    let video_monitor = tokio::spawn(routes::monitor_video_recording(state.clone()));
    let cleanup_state = state.clone();
    let app = dioxus::server::router(app)
        .nest("/api", routes::api_router(state))
        .layer(tower_http::trace::TraceLayer::new_for_http());

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
    let cleanup = async {
        workflow_agent.shutdown().await;
        workflow.shutdown().await;
        training.shutdown().await;
        rollout.shutdown().await;
        if let Err(error) = hardware.shutdown().await {
            tracing::error!("hardware shutdown did not verify every motor release: {error}");
        }
        routes::finish_stale_video_capture(&cleanup_state).await;
        let camera_result = tokio::task::spawn_blocking({
            let cameras = cameras.clone();
            move || cameras.shutdown()
        })
        .await;
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
    video_monitor.abort();
    bootstrap.abort();
    signal_task.abort();
    if let Some(result) = server_result {
        result?;
    }
    Ok(())
}

fn validate_bind_address(bind: SocketAddr, allow_remote_access: bool) -> Result<()> {
    if bind.ip().is_loopback() || allow_remote_access {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!(
            "refusing to expose the unauthenticated Studio control API on {bind}; use a loopback address or pass --allow-remote-access explicitly"
        ),
    )
    .into())
}

/// Warm external services and hydrate durable state after the HTTP listener
/// is already accepting requests. Catalog and camera I/O are independent and
/// therefore progress concurrently.
async fn bootstrap_services(state: AppState, discover_cameras: bool) {
    let catalog = bootstrap_catalog(state.clone());
    let camera = async {
        if !discover_cameras {
            return;
        }
        let cameras = state.cameras.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || cameras.refresh_blocking()).await {
            let message = format!("camera discovery task stopped: {error}");
            tracing::warn!("{message}");
            state.cameras.note_issue(message);
        }
    };
    tokio::join!(catalog, camera);
}

async fn bootstrap_catalog(state: AppState) {
    state
        .bootstrap
        .bootstrapping("Connecting to SpiralDB and warming the shared ADBC pool");
    if let Err(error) = state.storage.prewarm().await {
        tracing::warn!("could not prewarm SpiralDB during startup: {error}");
        state.bootstrap.degraded(false, error);
        return;
    }

    state
        .bootstrap
        .bootstrapping("Loading Studio preferences and run history");
    let preferences = {
        let state = state.clone();
        async move {
            let preferences_storage = state.storage.clone();
            let loaded =
                tokio::task::spawn_blocking(move || preferences_storage.load_studio_preferences())
                    .await;
            let issue = match loaded {
                Ok(Ok(preferences)) => {
                    state.preferences.replace(preferences);
                    None
                }
                Ok(Err(error)) => Some(format!("could not load Studio preferences: {error}")),
                Err(error) => Some(format!("Studio preferences task stopped: {error}")),
            };
            // Runtime selection lives in project preferences. Start both
            // readiness probes as soon as those preferences resolve, while
            // the independent history loads may still be running.
            state.training.refresh_readiness().await;
            issue
        }
    };
    let training = state.training.hydrate_from_storage();
    let workflow = state.workflow.hydrate_from_storage();
    let workflow_agent = state.workflow_agent.hydrate_from_storage();
    let rollout = {
        let rollout = state.rollout.clone();
        tokio::task::spawn_blocking(move || rollout.reload_from_storage())
    };
    let (preferences, training, workflow, workflow_agent, rollout) =
        tokio::join!(preferences, training, workflow, workflow_agent, rollout);

    let mut issues = Vec::new();
    if let Some(issue) = preferences {
        issues.push(issue);
    }
    if let Err(error) = training {
        issues.push(format!("could not load training history: {error}"));
    }
    if let Err(error) = workflow {
        issues.push(format!("could not load Workflow history: {error}"));
    }
    if let Err(error) = workflow_agent {
        issues.push(format!(
            "could not load Workflow Assistant history: {error}"
        ));
    }
    match rollout {
        Ok(Ok(())) => {}
        Ok(Err(error)) => issues.push(format!("could not load rollout history: {error}")),
        Err(error) => issues.push(format!("rollout history task stopped: {error}")),
    }
    if issues.is_empty() {
        state.bootstrap.ready();
    } else {
        let detail = issues.join("; ");
        tracing::warn!("Studio started with reduced history/preferences state: {detail}");
        state.bootstrap.degraded(true, detail);
    }
    routes::auto_connect_saved_robots(state).await;
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

    #[test]
    fn remote_control_bind_requires_explicit_opt_in() {
        let loopback = "127.0.0.1:8080".parse().unwrap();
        let remote = "0.0.0.0:8080".parse().unwrap();

        assert!(validate_bind_address(loopback, false).is_ok());
        assert!(validate_bind_address(remote, false).is_err());
        assert!(validate_bind_address(remote, true).is_ok());
    }

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
