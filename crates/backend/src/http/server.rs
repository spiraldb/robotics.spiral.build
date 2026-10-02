//! Native HTTP runtime for the Spiral Robotics backend, started from the
//! command line.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::account::SpiralAccount;
use crate::projects::context::StudioContext;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use clap::Parser;
use spiral_robotics_hardware::{
    CameraRuntime, ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS,
    HardwareHandle,
};
use spiral_robotics_storage::{
    BackendStateStore, ProviderKeyring, ServiceEndpoint, SpiralConnectionSettings,
};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Camera streams are intentionally long lived, so graceful draining
/// gets a short deadline instead of being allowed to hold process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Motor and camera cleanup normally completes within one control/frame cycle;
/// the final encoder segment and MCAP publication need time to finish.
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_VIDEO_CODEC: &str = "libx264";

/// Set up, calibrate, and drive SO-ARM101 arms from a browser.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Set up, calibrate, and drive SO-ARM101 arms from a browser",
    long_about = "Serves the Spiral Robotics Studio web UI, owns the hardware behind it, and stores Studio state in local SQLite and datasets in SpiralDB."
)]
struct StudioServerArguments {
    #[command(flatten)]
    access: ServerAccessArguments,
    #[command(flatten)]
    video: VideoArguments,
    #[command(flatten)]
    motion: MotionArguments,
}

#[derive(Debug, clap::Args)]
struct ServerAccessArguments {
    /// Address the web UI is served on.
    #[arg(long, value_name = "ADDR:PORT")]
    bind: Option<SocketAddr>,

    /// Permit the unauthenticated Studio control API to listen beyond loopback.
    #[arg(long, requires = "bind")]
    allow_remote_access: bool,
}

#[derive(Debug, clap::Args)]
struct VideoArguments {
    /// Skip camera discovery entirely.
    #[arg(long)]
    no_cameras: bool,

    /// FFmpeg executable used by the backend for episode camera recording.
    #[arg(long, default_value = DEFAULT_FFMPEG, value_name = "PATH")]
    ffmpeg: String,

    /// FFmpeg video codec used for camera access units in MCAP recordings.
    #[arg(long, default_value = DEFAULT_VIDEO_CODEC, value_name = "CODEC")]
    video_codec: String,
}

#[derive(Debug, clap::Args)]
struct MotionArguments {
    /// Largest change, in encoder counts, applied to a joint per control cycle.
    #[arg(long, default_value_t = DEFAULT_MAX_STEP_COUNTS, value_name = "COUNTS")]
    max_step: i64,

    /// Largest per-cycle change used by the teleoperation follower.
    #[arg(long, default_value_t = DEFAULT_TELEOP_MAX_STEP_COUNTS, value_name = "COUNTS")]
    teleop_max_step: i64,
}

impl MotionArguments {
    fn control_settings(&self) -> Result<ControlSettings> {
        if self.max_step <= 0 {
            return Err(std::io::Error::other("--max-step must be greater than zero").into());
        }
        if self.teleop_max_step <= 0 {
            return Err(
                std::io::Error::other("--teleop-max-step must be greater than zero").into(),
            );
        }
        Ok(ControlSettings::default().with_motion_limits(self.max_step, self.teleop_max_step)?)
    }
}

/// Everything a Studio server needs before it listens: its address, its
/// browser bundle and state, and its hardware settings.
struct ServerStartup {
    bind: SocketAddr,
    video: VideoArguments,
    camera_access_error: Option<String>,
    backend_state: Arc<BackendStateStore>,
    control_settings: ControlSettings,
    public: PathBuf,
}

/// Command-line Studio process launcher.
pub struct StudioServer;

impl StudioServer {
    pub fn run() -> ExitCode {
        // Camera probing re-executes this binary as an isolated worker, so this
        // must come before argument parsing.
        match CameraRuntime::run_worker_from_environment() {
            Ok(true) => return ExitCode::SUCCESS,
            Ok(false) => {}
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        }

        let arguments = StudioServerArguments::parse();
        let Some(public) = ServerStartup::dioxus_public_directory()
            .filter(|path| path.join("index.html").is_file())
        else {
            eprintln!(
                "error: the Dioxus client bundle is missing; from the workspace root, run `dx serve --fullstack --debug-symbols false @client --package spiral-robotics-frontend @server --package spiral-robotics-backend`, or use `dx bundle` with the same options and launch the bundled server"
            );
            return ExitCode::FAILURE;
        };
        ServerStartup::install_logging();
        tracing::debug!("Dioxus public directory: {}", public.display());

        let control_settings = match arguments.motion.control_settings() {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        // `dx serve` publishes its chosen address through the standard fullstack
        // environment. An explicit CLI value still wins for standalone launches.
        let bind = arguments
            .access
            .bind
            .unwrap_or_else(dioxus_cli_config::fullstack_address_or_localhost);
        if let Err(error) = arguments.access.validate_bind_address(bind) {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
        let backend_state = match BackendStateStore::open_default() {
            Ok(state) => Arc::new(state),
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let camera_access_error = if arguments.video.no_cameras {
            None
        } else {
            ServerStartup::request_camera_access()
        };

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

        let startup = ServerStartup {
            bind,
            video: arguments.video,
            camera_access_error,
            backend_state,
            control_settings,
            public,
        };
        match runtime.block_on(startup.serve_until_ctrl_c()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

impl ServerStartup {
    fn install_logging() {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    EnvFilter::new("spiral_robotics_backend=info,tower_http=warn")
                }),
            )
            .init();
    }

    /// On macOS this puts up the system camera prompt, which has to happen on
    /// the main thread before any runtime starts. Returns why camera previews
    /// will stay blank, if they will.
    fn request_camera_access() -> Option<String> {
        let error = CameraRuntime::initialize_access().err()?.to_string();
        tracing::warn!("camera access unavailable: {error}");
        Some(error)
    }

    /// Serve until the first Ctrl+C, then release the hardware.
    async fn serve_until_ctrl_c(self) -> Result<()> {
        let bound = self.bind().await?;
        println!(
            "Spiral Robotics Studio is running at http://{}",
            bound.address
        );
        let (shutdown, requested) = watch::channel(false);
        let signal_task = tokio::spawn(ShutdownSignal::listen(shutdown));
        let served = bound.serve(requested).await;
        signal_task.abort();
        served
    }

    /// Open Studio's state, account, and hardware, and listen on its address.
    async fn bind(self) -> Result<BoundServer> {
        let Self {
            bind,
            video,
            camera_access_error,
            backend_state,
            control_settings,
            public,
        } = self;
        let (saved, keys) = backend_state
            .initialize()
            .await
            .map_err(std::io::Error::other)?;
        let keys = Arc::new(ProviderKeyring::new(backend_state.clone(), keys));
        if let Some(path) = backend_state.studio().path() {
            tracing::info!(path = %path.display(), "Studio state is stored in SQLite");
        }
        let spiral_endpoint = ServiceEndpoint::spiral(
            &saved
                .spiral_endpoint()
                .unwrap_or_else(|| ServiceEndpoint::environment_spiral().into_string()),
        )
        .map_err(std::io::Error::other)?
        .into_string();
        let spiral_spfs_endpoint = ServiceEndpoint::spiral_spfs(
            &saved
                .spiral_spfs_endpoint()
                .unwrap_or_else(|| ServiceEndpoint::environment_spiral_spfs().into_string()),
        )
        .map_err(std::io::Error::other)?
        .into_string();
        let mut connection = SpiralConnectionSettings::new(spiral_endpoint, spiral_spfs_endpoint)
            .map_err(std::io::Error::other)?;
        if let Some(project) = saved.project_id() {
            connection = connection
                .with_project(&project)
                .map_err(std::io::Error::other)?;
        }
        let (account, spiral_access) = SpiralAccount::restore(connection.endpoint())
            .await
            .map_err(std::io::Error::other)?;
        let hardware = Arc::new(HardwareHandle::spawn(control_settings));
        let cameras = Arc::new(CameraManager::new());
        if let Some(error) = camera_access_error {
            cameras.note_issue(format!("camera previews will stay blank: {error}"));
        }
        let resources = crate::projects::ProjectHardware::new(
            hardware,
            cameras,
            VideoEncoderSettings::new(video.ffmpeg, video.video_codec),
        );
        let factory = crate::projects::ProjectFactory::new(
            resources,
            crate::projects::ProjectDataAccess::new(
                backend_state.studio().clone(),
                spiral_access,
                keys.subscribe(),
            ),
        );
        let projects = crate::projects::ProjectRegistry::new(factory, backend_state, connection);
        let state = StudioContext::new(projects, account, keys);
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let address = listener.local_addr()?;

        let bootstrap = tokio::spawn(
            crate::projects::bootstrap::ProjectBootstrap::new(state.clone()).run(!video.no_cameras),
        );
        let app = super::web_assets::StudioWebAssets::new(public)
            .router(super::routes::StudioApi::router(state.clone()))
            .layer(tower_http::trace::TraceLayer::new_for_http());
        Ok(BoundServer {
            listener,
            address,
            app,
            state,
            bootstrap,
        })
    }

    /// DX sets the public path during development; a distributed server keeps
    /// its browser bundle in the adjacent public directory.
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
}

/// A Studio server listening on its address, and the state it serves.
struct BoundServer {
    listener: tokio::net::TcpListener,
    address: SocketAddr,
    app: axum::Router,
    state: StudioContext,
    bootstrap: tokio::task::JoinHandle<()>,
}

impl BoundServer {
    /// Serve until `shutdown` turns true, then release motors and cameras.
    async fn serve(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        let Self {
            listener,
            app,
            state,
            bootstrap,
            ..
        } = self;
        // Both the server and this task observe the same edge. The server begins
        // graceful draining immediately; this task enforces a deadline because
        // Camera connections are designed not to finish on their own.
        let server_result = {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(ShutdownSignal::requested(shutdown.clone()))
                .into_future();
            tokio::pin!(server);

            tokio::select! {
                result = &mut server => Some(result),
            () = ShutdownSignal::requested(shutdown) => {
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
            let opened = state.opened();
            state.projects().shutdown().await;
            if let Err(error) = state.projects().hardware().shutdown().await {
                tracing::error!("hardware shutdown did not verify every motor release: {error}");
            }
            for project in &opened {
                crate::recording::take::TakeOperations::new(project)
                    .close_ended(None)
                    .await;
            }
            let camera_result = tokio::task::spawn_blocking({
                let cameras = state.cameras().clone();
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
        bootstrap.abort();
        if let Some(result) = server_result {
            result?;
        }
        Ok(())
    }
}

impl ServerAccessArguments {
    fn validate_bind_address(&self, bind: SocketAddr) -> Result<()> {
        if bind.ip().is_loopback() || self.allow_remote_access {
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
}

/// Notify every shutdown waiter on the first Ctrl+C and make the second press
/// an unconditional escape hatch. Once Tokio installs its signal handler the
/// operating system's default second-press behavior no longer applies, so it
/// has to be restored explicitly.
struct ShutdownSignal;

impl ShutdownSignal {
    async fn listen(shutdown: tokio::sync::watch::Sender<bool>) {
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

    async fn requested(mut shutdown: tokio::sync::watch::Receiver<bool>) {
        if *shutdown.borrow() {
            return;
        }
        while shutdown.changed().await.is_ok() {
            if *shutdown.borrow() {
                return;
            }
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

        let local_only = ServerAccessArguments {
            bind: None,
            allow_remote_access: false,
        };
        let remote_allowed = ServerAccessArguments {
            bind: None,
            allow_remote_access: true,
        };

        assert!(local_only.validate_bind_address(loopback).is_ok());
        assert!(local_only.validate_bind_address(remote).is_err());
        assert!(remote_allowed.validate_bind_address(remote).is_ok());
    }

    #[tokio::test]
    async fn shutdown_waiters_release_when_the_signal_is_published() {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(ShutdownSignal::requested(receiver));

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
            ShutdownSignal::requested(receiver),
        )
        .await
        .expect("an existing shutdown request is immediately visible");
    }
}
