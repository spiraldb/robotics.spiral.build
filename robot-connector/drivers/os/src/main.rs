//! The robot connector for computers — Mac, Linux, and Raspberry Pi — started
//! from the command line.
//!
//! The connector itself is the shared library; this program supplies a
//! computer's device drivers: USB serial ports, webcams, and FFmpeg.

mod cameras;
mod ffmpeg;
mod serial_ports;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use spiral_robotics_connector::{
    CameraDriver, ConnectorSettings, DeviceDrivers, NoCameraDriver, RobotConnector,
};
use spiral_robotics_hardware::{
    ControlSettings, DEFAULT_MAX_STEP_COUNTS, DEFAULT_TELEOP_MAX_STEP_COUNTS,
};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use crate::cameras::{CameraRuntime, SystemCameras};
use crate::ffmpeg::FfmpegEncoder;
use crate::serial_ports::SystemSerialPorts;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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
struct ConnectorStartup {
    bind: SocketAddr,
    demo: bool,
    drivers: DeviceDrivers,
    settings: ConnectorSettings,
    camera_access_error: Option<String>,
}

fn main() -> ExitCode {
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
    ConnectorStartup::install_logging();
    let startup = match ConnectorStartup::from_arguments(arguments) {
        Ok(startup) => startup,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
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
    match runtime.block_on(startup.serve_until_ctrl_c()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

impl ConnectorStartup {
    fn install_logging() {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("spiral_robotics_connector=info,tower_http=warn")
            }))
            .init();
    }

    fn from_arguments(arguments: RobotConnectorArguments) -> Result<Self> {
        let RobotConnectorArguments {
            bind,
            data_dir,
            policy_runtime,
            demo,
            video,
            motion,
        } = arguments;
        let control = motion.control_settings()?;
        let data_dir = data_dir
            .or_else(|| Self::default_data_dir(demo))
            .ok_or("cannot locate a data directory; pass --data-dir")?;
        // A demo films its simulated arms only.
        let look_for_cameras = !video.no_cameras && !demo;
        let camera_access_error = if look_for_cameras {
            Self::request_camera_access()
        } else {
            None
        };
        let cameras: Arc<dyn CameraDriver> = if look_for_cameras {
            Arc::new(SystemCameras)
        } else {
            Arc::new(NoCameraDriver)
        };
        let drivers = DeviceDrivers::new(
            Arc::new(SystemSerialPorts),
            cameras,
            Arc::new(FfmpegEncoder::new(video.ffmpeg, video.video_codec)?),
        );
        Ok(Self {
            bind,
            demo,
            drivers,
            settings: ConnectorSettings::new(data_dir, policy_runtime, control),
            camera_access_error,
        })
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
        let Self {
            bind,
            demo,
            drivers,
            settings,
            camera_access_error,
        } = self;
        let connector = RobotConnector::open(drivers, settings)?;
        if demo {
            connector.install_demo_bench().await?;
        }
        if let Some(error) = camera_access_error {
            connector.note_camera_issue(format!("camera previews will stay blank: {error}"));
        }
        let listener = tokio::net::TcpListener::bind(bind).await?;
        println!(
            "Spiral Robotics robot connector is running at http://{}",
            listener.local_addr()?
        );
        let (shutdown, requested) = watch::channel(false);
        let signal_task = tokio::spawn(CtrlC::listen(shutdown));
        let served = connector.serve(listener, requested).await;
        signal_task.abort();
        served
    }
}

/// Ctrl+C asks the connector to stop; a second press is an unconditional
/// escape hatch. Once Tokio installs its signal handler the operating
/// system's default second-press behavior no longer applies, so it has to be
/// restored explicitly.
struct CtrlC;

impl CtrlC {
    async fn listen(shutdown: watch::Sender<bool>) {
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
}
