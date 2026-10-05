//! The simulated arms as a scene: each posed as the control loop last measured it.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use spiral_robotics_common::robots::config::{CameraConfig, MotorAccess};
use spiral_robotics_common::robots::visualization::RobotModelId;
use spiral_robotics_hardware::HardwareHandle;
use spiral_robotics_viewport::{ArmPresentation, CameraAttachment, CameraMount, Frame, RobotModel};

use super::SimulatedCamera;
use crate::robots::models::RobotModelStore;
use crate::robots::workspace::{RobotRecord, RobotWorkspace};

/// The saved robots whose motors are simulated, and the control loop moving them.
#[derive(Clone)]
pub(crate) struct SimulatedWorld {
    workspace: Arc<RobotWorkspace>,
    hardware: Arc<HardwareHandle>,
}

impl SimulatedWorld {
    pub(crate) fn new(workspace: Arc<RobotWorkspace>, hardware: Arc<HardwareHandle>) -> Self {
        Self {
            workspace,
            hardware,
        }
    }

    /// Every mounted camera on a simulated arm that has a model to film.
    pub(crate) fn cameras(&self) -> Vec<SimulatedCamera> {
        self.arms()
            .iter()
            .flat_map(|arm| {
                arm.config()
                    .cameras()
                    .iter()
                    .filter(|(_, camera)| camera.mount().is_some())
                    .map(|(role, camera)| {
                        SimulatedCamera::new(self.clone(), (arm.id(), role), camera.id())
                    })
            })
            .collect()
    }

    /// The bench as it stands now, parsing each model at most once per `models`.
    pub(crate) fn frame(&self, models: &mut ParsedModels) -> Result<Frame, String> {
        let hardware = self.hardware.state();
        let mut frame = Frame::new(true);
        for arm in self.arms() {
            let visualization = arm
                .config()
                .visualization()
                .expect("a simulated arm in the scene has a model");
            let model = models.parse(self.workspace.models(), visualization.model())?;
            // Uncalibrated telemetry has no angles to pose with; such an arm rests.
            let joints = hardware
                .telemetry()
                .robots()
                .iter()
                .find(|robot| robot.id() == arm.id() && arm.config().calibrated())
                .map(|robot| robot.joints())
                .unwrap_or_default();
            frame
                .add_arm(
                    arm.id(),
                    ArmPresentation::new(model, visualization.measured_positions(joints)),
                )
                .map_err(|error| error.to_string())?;
            for (role, camera) in arm.config().cameras() {
                if let Some(mount) = camera.mount_in_scene(role) {
                    frame
                        .mount_camera(arm.id(), mount)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(frame)
    }

    /// Simulated robots with a model, in the order the browser's bench lays
    /// them out, so both put each arm in the same place.
    fn arms(&self) -> Vec<RobotRecord> {
        self.workspace
            .records()
            .into_iter()
            .filter(|record| {
                matches!(
                    record.config().motor_access(),
                    Some(MotorAccess::Simulated(_))
                ) && record.config().visualization().is_some()
            })
            .collect()
    }
}

/// Robot models parsed on one render thread, kept for as long as it runs so
/// their meshes are uploaded once.
#[derive(Default)]
pub(crate) struct ParsedModels(BTreeMap<RobotModelId, Rc<RobotModel>>);

impl ParsedModels {
    fn parse(
        &mut self,
        store: &RobotModelStore,
        id: &RobotModelId,
    ) -> Result<Rc<RobotModel>, String> {
        if let Some(model) = self.0.get(id) {
            return Ok(model.clone());
        }
        let files = store.load(id).map_err(|error| error.to_string())?;
        let model = Rc::new(RobotModel::parse_encoded(files.urdf(), files.meshes())?);
        self.0.insert(id.clone(), model.clone());
        Ok(model)
    }
}

trait CameraMountScene {
    /// Where this camera stands in a scene, when its configuration says.
    fn mount_in_scene(&self, role: &str) -> Option<CameraMount>;
}

impl CameraMountScene for CameraConfig {
    fn mount_in_scene(&self, role: &str) -> Option<CameraMount> {
        let mount = self.mount()?;
        Some(CameraMount::new(
            role,
            CameraAttachment::new(mount.frame(), mount.xyz(), mount.rpy()),
            mount.fov_degrees(),
        ))
    }
}
