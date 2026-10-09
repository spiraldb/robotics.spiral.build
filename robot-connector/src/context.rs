//! What a connector request can reach: the devices this machine drives, and
//! what the connector keeps for Studio — saved robots, the take being recorded,
//! finished recordings, and policies.
use std::path::Path;
use std::sync::Arc;

use spiral_robotics_hardware::{HardwareHandle, SerialDriver};
use spiral_robotics_recording::RecordingBuffer;

use crate::policies::PolicyStore;
use crate::recording::files::RecordingFiles;
use crate::recording::session::RecordingSession;
use crate::robotics::cameras::CameraManager;
use crate::robotics::video::EncoderDriver;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::{RolloutHardwareResources, RolloutManager};

/// The physical devices one connector process owns.
pub(crate) struct RobotConnectorDevices {
    hardware: Arc<HardwareHandle>,
    serial_driver: Arc<dyn SerialDriver>,
    cameras: Arc<CameraManager>,
    encoder_driver: Arc<dyn EncoderDriver>,
}

impl RobotConnectorDevices {
    pub(crate) fn new(
        hardware: Arc<HardwareHandle>,
        serial_driver: Arc<dyn SerialDriver>,
        cameras: Arc<CameraManager>,
        encoder_driver: Arc<dyn EncoderDriver>,
    ) -> Self {
        Self {
            hardware,
            serial_driver,
            cameras,
            encoder_driver,
        }
    }
}

/// The one take a connector records, and the files finished takes leave.
struct ConnectorTakes {
    buffer: Arc<RecordingBuffer>,
    session: Arc<RecordingSession>,
    files: Arc<RecordingFiles>,
}

struct ConnectorState {
    devices: RobotConnectorDevices,
    workspace: Arc<RobotWorkspace>,
    takes: ConnectorTakes,
    policies: Arc<PolicyStore>,
    rollout: Arc<RolloutManager>,
}

/// Every request and background task shares one connector.
#[derive(Clone)]
pub(crate) struct RobotConnectorContext(Arc<ConnectorState>);

impl RobotConnectorContext {
    /// Serve `workspace`'s robots and the takes and policies kept under
    /// `data_dir`.
    pub(crate) fn open(
        devices: RobotConnectorDevices,
        workspace: Arc<RobotWorkspace>,
        data_dir: &Path,
    ) -> Self {
        let policies = Arc::new(PolicyStore::new(data_dir.join("policies")));
        let rollout = RolloutManager::new(
            RolloutHardwareResources::new(
                devices.hardware.clone(),
                devices.cameras.clone(),
                workspace.clone(),
            ),
            policies.clone(),
        );
        Self(Arc::new(ConnectorState {
            devices,
            workspace,
            takes: ConnectorTakes {
                buffer: Arc::new(RecordingBuffer::new()),
                session: Arc::new(RecordingSession::default()),
                files: Arc::new(RecordingFiles::new(data_dir.join("recordings"))),
            },
            policies,
            rollout,
        }))
    }

    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        &self.0.devices.hardware
    }
    pub(crate) fn serial_driver(&self) -> &Arc<dyn SerialDriver> {
        &self.0.devices.serial_driver
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        &self.0.devices.cameras
    }
    pub(crate) fn encoder_driver(&self) -> &Arc<dyn EncoderDriver> {
        &self.0.devices.encoder_driver
    }
    pub(crate) fn workspace(&self) -> &Arc<RobotWorkspace> {
        &self.0.workspace
    }
    pub(crate) fn recording_buffer(&self) -> &Arc<RecordingBuffer> {
        &self.0.takes.buffer
    }
    pub(crate) fn recording_session(&self) -> &Arc<RecordingSession> {
        &self.0.takes.session
    }
    pub(crate) fn recording_files(&self) -> &Arc<RecordingFiles> {
        &self.0.takes.files
    }
    pub(crate) fn policies(&self) -> &Arc<PolicyStore> {
        &self.0.policies
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        &self.0.rollout
    }
}
