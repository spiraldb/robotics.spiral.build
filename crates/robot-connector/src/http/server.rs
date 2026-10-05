//! Native HTTP runtime for the robot connector, started from the command line.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::context::{RobotConnectorContext, RobotConnectorDevices};
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::VideoEncoderSettings;
use crate::robots::models::RobotModelStore;
use crate::robots::workspace::RobotWorkspace;
use crate::simulation::{DemoBench, SimulatedWorld};
use clap::Parser;
use spiral_robotics_hardware::{
    CameraRuntime, ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS,
    HardwareHandle,
};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Camera streams are intentionally long lived, so graceful draining
/// gets a short deadline instead of being allowed to hold process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Motor and camera cleanup normally completes within one control/frame cycle;
/// the final encoder segment and recording file need time to finish.
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const DEFAULT_BIND: &str = "127.0.0.1:7070";
const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_VIDEO_CODEC: &str = "libx264";
const DEFAULT_POLICY_RUNTIME: &str = "http://127.0.0.1:50051";

/// Drive SO-ARM101 arms for Spiral Robotics Studio.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Drive SO-ARM101 arms for Spiral Robotics Studio",
    long_about = "Runs next to the robot arms: owns their serial buses and cameras, records takes, and rolls out policies. Studio, in a browser, connects to this server."
)]
struct RobotConnectorArguments {
    /// Address the robot connector API listens on.
    #[arg(long, default_value = DEFAULT_BIND, value_name = "ADDR:PORT")]
    bind: SocketAddr,

    /// Where saved robots, finished recordings, and policies live.
    #[arg(
        long,
        env = "SPIRAL_ROBOTICS_ROBOT_CONNECTOR_DATA",
        value_name = "PATH"
    )]
    data_dir: Option<PathBuf>,

    /// Policy runtime that computes rollout actions.
    #[arg(long, default_value = DEFAULT_POLICY_RUNTIME, value_name = "URL")]
    policy_runtime: String,

    /// Start with a simulated SO-101 leader and follower, filmed by simulated
    /// cameras, instead of real arms. Keeps its own data directory unless
    /// `--data-dir` names one, and looks for no physical cameras.
    #[arg(long)]
    demo: bool,

    #[command(flatten)]
    video: VideoArguments,
    #[command(flatten)]
    motion: MotionArguments,
}

#[derive(Debug, clap::Args)]
struct VideoArguments {
    /// Skip camera discovery entirely.
    #[arg(long)]
    no_cameras: bool,

    /// FFmpeg executable used for episode camera recording.
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

/// Everything the connector needs before it listens.
struct ServerStartup {
    bind: SocketAddr,
    data_dir: PathBuf,
    policy_runtime: String,
    demo: bool,
    video: VideoArguments,
    camera_access_error: Option<String>,
    control_settings: ControlSettings,
}

/// Command-line robot connector process launcher.
pub struct RobotConnectorServer;

impl RobotConnectorServer {
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

        let arguments = RobotConnectorArguments::parse();
        ServerStartup::install_logging();
        let control_settings = match arguments.motion.control_settings() {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let Some(data_dir) = arguments
            .data_dir
            .or_else(|| ServerStartup::default_data_dir(arguments.demo))
        else {
            eprintln!("error: cannot locate a data directory; pass --data-dir");
            return ExitCode::FAILURE;
        };
        let camera_access_error = if arguments.video.no_cameras || arguments.demo {
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
            bind: arguments.bind,
            data_dir,
            policy_runtime: arguments.policy_runtime,
            demo: arguments.demo,
            video: arguments.video,
            camera_access_error,
            control_settings,
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
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("spiral_robotics_robot_connector=info,tower_http=warn")
            }))
            .init();
    }

    /// A demo keeps its simulated arms and takes apart from real ones.
    fn default_data_dir(demo: bool) -> Option<PathBuf> {
        let name = if demo {
            "robot-connector-demo"
        } else {
            "robot-connector"
        };
        Some(dirs::data_local_dir()?.join("spiral-robotics").join(name))
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
            "Spiral Robotics robot connector is running at http://{}",
            bound.address
        );
        let (shutdown, requested) = watch::channel(false);
        let signal_task = tokio::spawn(ShutdownSignal::listen(shutdown));
        let served = bound.serve(requested).await;
        signal_task.abort();
        served
    }

    /// Open the connector's saved state and hardware, and listen on its address.
    async fn bind(self) -> Result<BoundServer> {
        let Self {
            bind,
            data_dir,
            policy_runtime,
            demo,
            video,
            camera_access_error,
            control_settings,
        } = self;
        tracing::info!(path = %data_dir.display(), "robot connector data directory");
        let hardware = Arc::new(HardwareHandle::spawn(control_settings));
        let workspace = Arc::new(RobotWorkspace::open(
            data_dir.join("robots.json"),
            RobotModelStore::new(data_dir.join("models")),
        )?);
        if demo {
            DemoBench::install(&workspace).await?;
        }
        let cameras = Arc::new(CameraManager::new(SimulatedWorld::new(
            workspace.clone(),
            hardware.clone(),
        )));
        if let Some(error) = camera_access_error {
            cameras.note_issue(format!("camera previews will stay blank: {error}"));
        }
        let devices = RobotConnectorDevices::new(
            hardware,
            cameras.clone(),
            VideoEncoderSettings::new(video.ffmpeg, video.video_codec),
        );
        let connector = RobotConnectorContext::open(devices, workspace, &data_dir, policy_runtime);
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let address = listener.local_addr()?;

        let discover_cameras = !video.no_cameras && !demo;
        let bootstrap = tokio::spawn({
            let connector = connector.clone();
            async move {
                if discover_cameras {
                    crate::robotics::cameras::CameraDiscovery::new(cameras)
                        .run()
                        .await;
                }
                crate::robots::operations::RobotOperations::new(&connector)
                    .auto_connect_saved()
                    .await;
            }
        });
        let app = super::routes::RobotConnectorApi::router(connector.clone())
            .layer(tower_http::trace::TraceLayer::new_for_http());
        Ok(BoundServer {
            listener,
            address,
            app,
            connector,
            bootstrap,
        })
    }
}

/// A connector listening on its address, and the state it serves.
struct BoundServer {
    listener: tokio::net::TcpListener,
    address: SocketAddr,
    app: axum::Router,
    connector: RobotConnectorContext,
    bootstrap: tokio::task::JoinHandle<()>,
}

impl BoundServer {
    /// Serve until `shutdown` turns true, then release motors and cameras.
    async fn serve(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        let Self {
            listener,
            app,
            connector,
            bootstrap,
            ..
        } = self;
        // Both the server and this task observe the same edge. The server begins
        // graceful draining immediately; this task enforces a deadline because
        // camera connections are designed not to finish on their own.
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
            connector.rollout().shutdown().await;
            if let Err(error) = connector.hardware().shutdown().await {
                tracing::error!("hardware shutdown did not verify every motor release: {error}");
            }
            crate::recording::take::TakeOperations::new(&connector)
                .close_ended(None)
                .await;
            let camera_result = tokio::task::spawn_blocking({
                let cameras = connector.cameras().clone();
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
