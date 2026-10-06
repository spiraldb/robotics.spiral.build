//! What has to be true before a checkpoint may drive the follower.

use spiral_robotics_common::robots::RobotSummary;

use super::checkpoints::CheckpointChoice;
use crate::robots::RobotSetupSnapshot;

/// The bench as a policy run needs it: a finished checkpoint, a connected and
/// calibrated follower, every camera the policy sees, and fresh telemetry.
pub(in crate::activities::rollout) struct PolicyReadiness<'a> {
    checkpoint: Option<&'a CheckpointChoice>,
    setup: &'a RobotSetupSnapshot,
    controls_available: bool,
}

impl<'a> PolicyReadiness<'a> {
    pub(in crate::activities::rollout) fn new(
        checkpoint: Option<&'a CheckpointChoice>,
        setup: &'a RobotSetupSnapshot,
        controls_available: bool,
    ) -> Self {
        Self {
            checkpoint,
            setup,
            controls_available,
        }
    }

    /// The first thing standing in the way, said as what to do about it.
    pub(in crate::activities::rollout) fn blocker(&self) -> Option<String> {
        // The newest checkpoint is chosen by default, so none chosen means
        // none exist.
        let Some(checkpoint) = self.checkpoint else {
            return Some("Train a policy to create a checkpoint.".to_owned());
        };
        if checkpoint.run_active() {
            return Some("Stop or finish training before running this checkpoint.".to_owned());
        }
        let Some(follower) = self.setup.follower() else {
            return Some("Set up a follower to run a policy.".to_owned());
        };
        if !follower.connection().connected() {
            return Some("Connect the follower.".to_owned());
        }
        if !follower.config().calibrated() {
            return Some("Calibrate the follower.".to_owned());
        }
        if let Some(camera) = self.camera_blocker(follower, checkpoint) {
            return Some(camera);
        }
        if !self.controls_available {
            return Some("Waiting for fresh telemetry.".to_owned());
        }
        None
    }

    /// The policy sees the follower's cameras it was trained on, or all of
    /// them when it names none; each needs a stream that is answering.
    fn camera_blocker(
        &self,
        follower: &RobotSummary,
        checkpoint: &CheckpointChoice,
    ) -> Option<String> {
        let assigned = follower.config().cameras();
        let roles = if checkpoint.camera_roles().is_empty() {
            assigned.keys().cloned().collect()
        } else {
            checkpoint.camera_roles().to_vec()
        };
        if roles.is_empty() {
            return Some("Assign the follower a camera for the policy.".to_owned());
        }
        let missing = roles
            .iter()
            .filter(|role| {
                !assigned.get(role.as_str()).is_some_and(|camera| {
                    self.setup
                        .cameras()
                        .iter()
                        .any(|status| status.id() == camera.id() && status.error().is_none())
                })
            })
            .collect::<Vec<_>>();
        match missing.as_slice() {
            [] => None,
            [role] => Some(format!("The policy's {role} camera is unavailable.")),
            _ => Some(format!(
                "{} of the policy's {} cameras are unavailable.",
                missing.len(),
                roles.len()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::robots::config::{MotorRange, RobotConfig};
    use spiral_robotics_common::robots::{
        CameraStatus, CameraStreamFormat, CameraStreamState, ConnectionStatus, ModeStatus,
        RobotSummary,
    };
    use spiral_robotics_common::training::{
        CheckpointPolicy, TrainingCheckpoint, TrainingConfig, TrainingInputSummary, TrainingRun,
        TrainingRunIdentity, TrainingRunPhase, TrainingRunStatus,
    };

    use super::PolicyReadiness;
    use crate::activities::rollout::policy::checkpoints::CheckpointChoice;
    use crate::robots::RobotSetupSnapshot;

    /// A connected, calibrated SO-101 follower with a gripper and a top camera.
    fn follower() -> RobotSummary {
        let config = RobotConfig::so101("follower", "so101_follower").unwrap();
        let range = MotorRange::new(0, 1, 2).unwrap();
        let ranges = config
            .motors()
            .keys()
            .map(|name| (name.clone(), range))
            .collect();
        let config = config
            .with_calibration(ranges)
            .and_then(|config| config.with_camera("gripper", "camera-1"))
            .and_then(|config| config.with_camera("top", "camera-2"))
            .unwrap();
        RobotSummary::new(config, 1, ConnectionStatus::new(true, None))
    }

    fn camera(id: &str, error: Option<&str>) -> CameraStatus {
        CameraStatus::new(id, id, CameraStreamFormat::new(640, 360, 30))
            .with_state(CameraStreamState::new(true, 1, error.map(str::to_owned)))
    }

    fn checkpoint(phase: TrainingRunPhase) -> CheckpointChoice {
        let run = TrainingRun::new(
            TrainingRunIdentity::new(
                "run",
                TrainingConfig::default(),
                TrainingInputSummary::default(),
                1,
            ),
            TrainingRunStatus::new(phase, 0, None),
        );
        let policy = CheckpointPolicy::new(
            vec![
                "follower.camera.gripper".to_owned(),
                "follower.camera.top".to_owned(),
            ],
            10,
            Some(30.0),
        );
        let checkpoints = [TrainingCheckpoint::new("run", 1_000).with_policy(policy)];
        CheckpointChoice::listed(&[run], &checkpoints).remove(0)
    }

    fn bench(cameras: Vec<CameraStatus>) -> RobotSetupSnapshot {
        RobotSetupSnapshot::new(ModeStatus::default())
            .with_robots(vec![follower()])
            .with_cameras(cameras)
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_policy_needs_every_camera_it_was_trained_on() {
        let finished = checkpoint(TrainingRunPhase::Completed);
        let ready = bench(vec![camera("camera-1", None), camera("camera-2", None)]);
        assert_eq!(
            PolicyReadiness::new(Some(&finished), &ready, true).blocker(),
            None
        );
        let one_offline = bench(vec![
            camera("camera-1", None),
            camera("camera-2", Some("offline")),
        ]);
        assert_eq!(
            PolicyReadiness::new(Some(&finished), &one_offline, true)
                .blocker()
                .as_deref(),
            Some("The policy's top camera is unavailable.")
        );
        assert_eq!(
            PolicyReadiness::new(Some(&finished), &bench(Vec::new()), true)
                .blocker()
                .as_deref(),
            Some("2 of the policy's 2 cameras are unavailable.")
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_checkpoint_still_training_waits_and_no_checkpoint_asks_for_training() {
        let bench = bench(vec![camera("camera-1", None), camera("camera-2", None)]);
        assert_eq!(
            PolicyReadiness::new(Some(&checkpoint(TrainingRunPhase::Running)), &bench, true)
                .blocker()
                .as_deref(),
            Some("Stop or finish training before running this checkpoint.")
        );
        assert_eq!(
            PolicyReadiness::new(None, &bench, true)
                .blocker()
                .as_deref(),
            Some("Train a policy to create a checkpoint.")
        );
    }
}
