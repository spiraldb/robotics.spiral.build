//! Open a robot connector on the machine's device drivers and serve its HTTP
//! API.
//!
//! The program around it — a computer's command line or a microcontroller's
//! firmware — supplies the device drivers, binds the listener, and decides
//! when to stop.

use std::path::PathBuf;
use std::sync::Arc;

use crate::context::{RobotConnectorContext, RobotConnectorDevices};
use crate::robotics::cameras::{CameraDriver, CameraManager};
use crate::robotics::video::EncoderDriver;
use crate::robots::models::RobotModelStore;
use crate::robots::workspace::RobotWorkspace;
#[cfg(feature = "simulation")]
use crate::simulation::SimulatedWorld;
use spiral_robotics_hardware::{HardwareHandle, SerialDriver};
use tokio::sync::watch;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Camera streams are intentionally long lived, so graceful draining
/// gets a short deadline instead of being allowed to hold process exit open.
const SERVER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);
/// Motor and camera cleanup normally completes within one control/frame cycle;
/// the final encoder segment and recording file need time to finish.
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

/// The device drivers of the machine running the connector: the serial
/// ports arms are wired to, its cameras, and an H.264 encoder for recordings.
/// A computer drives them through its operating system; a microcontroller
/// through its own peripherals.
pub struct DeviceDrivers {
    serial_driver: Arc<dyn SerialDriver>,
    camera_driver: Arc<dyn CameraDriver>,
    encoder_driver: Arc<dyn EncoderDriver>,
}

impl DeviceDrivers {
    pub fn new(
        serial_driver: Arc<dyn SerialDriver>,
        camera_driver: Arc<dyn CameraDriver>,
        encoder_driver: Arc<dyn EncoderDriver>,
    ) -> Self {
        Self {
            serial_driver,
            camera_driver,
            encoder_driver,
        }
    }
}

/// A connector with its control loop running and its saved state open,
/// ready to serve Studio.
pub struct RobotConnector(RobotConnectorContext);

impl RobotConnector {
    /// Start the control loop on `drivers` and open the saved robots, unsent
    /// takes, and policies kept under `data_dir`. Everything else a connector
    /// does is configured from Studio.
    pub fn open(drivers: DeviceDrivers, data_dir: PathBuf) -> Result<Self> {
        let DeviceDrivers {
            serial_driver,
            camera_driver,
            encoder_driver,
        } = drivers;
        tracing::info!(path = %data_dir.display(), "robot connector data directory");
        let hardware = Arc::new(HardwareHandle::spawn(serial_driver.clone()));
        let workspace = Arc::new(RobotWorkspace::open(
            data_dir.join("robots.json"),
            RobotModelStore::new(data_dir.join("models")),
        )?);
        // Arms with simulated motors are filmed by drawing them.
        #[cfg(feature = "simulation")]
        let simulated = Some(
            Arc::new(SimulatedWorld::new(workspace.clone(), hardware.clone()))
                as Arc<dyn CameraDriver>,
        );
        #[cfg(not(feature = "simulation"))]
        let simulated = None;
        let cameras = Arc::new(CameraManager::new(camera_driver, simulated));
        let devices = RobotConnectorDevices::new(hardware, serial_driver, cameras, encoder_driver);
        Ok(Self(RobotConnectorContext::open(
            devices, workspace, &data_dir,
        )))
    }

    /// Show the operator a camera problem discovery cannot detect, such as a
    /// denied permission prompt.
    pub fn note_camera_issue(&self, message: impl Into<String>) {
        self.0.cameras().note_issue(message);
    }

    /// Serve Studio on `listener` until `shutdown` turns true, then release
    /// motors and cameras.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let connector = self.0;
        // Cameras and saved robots come up once requests are already being
        // answered, so a slow probe never delays the API.
        let bootstrap = tokio::spawn({
            let connector = connector.clone();
            async move {
                crate::robotics::cameras::CameraDiscovery::new(connector.cameras().clone())
                    .run()
                    .await;
                crate::robots::operations::RobotOperations::new(&connector)
                    .auto_connect_saved()
                    .await;
            }
        });
        let app = super::routes::RobotConnectorApi::router(connector.clone())
            .layer(tower_http::trace::TraceLayer::new_for_http());

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

/// Waits for a published shutdown request.
struct ShutdownSignal;

impl ShutdownSignal {
    async fn requested(mut shutdown: watch::Receiver<bool>) {
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
