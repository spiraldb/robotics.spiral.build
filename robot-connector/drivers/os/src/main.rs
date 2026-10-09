//! The robot connector for computers — Mac, Linux, and Raspberry Pi — started
//! from the command line.
//!
//! The connector itself is the shared library; this program supplies a
//! computer's device drivers: USB serial ports, webcams, and FFmpeg.

mod cameras;
mod ffmpeg;
mod serial_ports;
mod studio_link;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use spiral_robotics_connector::{
    CameraDriver, DeviceDrivers, NoCameraDriver, RobotConnector, StudioLink,
};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

use crate::cameras::{CameraRuntime, SystemCameras};
use crate::ffmpeg::FfmpegEncoder;
use crate::serial_ports::SystemSerialPorts;
use crate::studio_link::HttpLink;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const DEFAULT_STUDIO: &str = "http://localhost:3000";
const DEFAULT_FFMPEG: &str = "ffmpeg";
const DEFAULT_VIDEO_CODEC: &str = "libx264";

/// Drive SO-ARM101 arms for Spiral Robotics Studio.
///
/// These options configure this computer's device drivers only; everything
/// the connector does is configured from Studio.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Drive SO-ARM101 arms for Spiral Robotics Studio",
    long_about = "Runs next to the robot arms: owns their serial buses and cameras, records takes, and rolls out policies. It links itself to Studio, which configures it and relays the browser to it."
)]
struct RobotConnectorArguments {
    /// The Studio to link to, by its web address.
    #[arg(long, env = "SPIRAL_ROBOTICS_STUDIO", default_value = DEFAULT_STUDIO, value_name = "URL")]
    studio: String,

    /// The token the workspace issued in Rollout → Set up.
    #[arg(
        long,
        env = "SPIRAL_ROBOTICS_CONNECTOR_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    token: String,

    /// Where saved robots, finished recordings, and policies live.
    #[arg(
        long,
        env = "SPIRAL_ROBOTICS_ROBOT_CONNECTOR_DATA",
        value_name = "PATH"
    )]
    data_dir: Option<PathBuf>,

    #[command(flatten)]
    video: VideoArguments,
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

/// Everything the connector needs before it listens.
struct ConnectorStartup {
    studio: StudioLink,
    data_dir: PathBuf,
    drivers: DeviceDrivers,
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
    match runtime.block_on(startup.run_until_ctrl_c()) {
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
            studio,
            token,
            data_dir,
            video,
        } = arguments;
        let studio = StudioLink::new(&studio, token)?;
        let data_dir = data_dir
            .or_else(Self::default_data_dir)
            .ok_or("cannot locate a data directory; pass --data-dir")?;
        let (camera_driver, camera_access_error): (Arc<dyn CameraDriver>, _) = if video.no_cameras {
            (Arc::new(NoCameraDriver), None)
        } else {
            (Arc::new(SystemCameras), Self::request_camera_access())
        };
        let drivers = DeviceDrivers::new(
            Arc::new(SystemSerialPorts),
            camera_driver,
            Arc::new(FfmpegEncoder::new(video.ffmpeg, video.video_codec)?),
            Arc::new(HttpLink::new()?),
        );
        Ok(Self {
            studio,
            data_dir,
            drivers,
            camera_access_error,
        })
    }

    fn default_data_dir() -> Option<PathBuf> {
        Some(
            dirs::data_local_dir()?
                .join("spiral-robotics")
                .join("robot-connector"),
        )
    }

    /// On macOS this puts up the system camera prompt, which has to happen on
    /// the main thread before any runtime starts. Returns why camera previews
    /// will stay blank, if they will.
    fn request_camera_access() -> Option<String> {
        let error = CameraRuntime::initialize_access().err()?.to_string();
        tracing::warn!("camera access unavailable: {error}");
        Some(error)
    }

    /// Run linked to Studio until the first Ctrl+C, then release the hardware.
    async fn run_until_ctrl_c(self) -> Result<()> {
        let Self {
            studio,
            data_dir,
            drivers,
            camera_access_error,
        } = self;
        let connector = RobotConnector::open(drivers, data_dir)?;
        if let Some(error) = camera_access_error {
            connector.note_camera_issue(format!("camera previews will stay blank: {error}"));
        }
        let (shutdown, requested) = watch::channel(false);
        let signal_task = tokio::spawn(CtrlC::listen(shutdown));
        let ran = connector.run(studio, requested).await;
        signal_task.abort();
        ran
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
