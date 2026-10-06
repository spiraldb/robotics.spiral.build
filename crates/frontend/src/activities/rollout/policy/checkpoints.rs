//! The checkpoints a policy can run from, read out of the training history.

use spiral_robotics_common::training::{TrainingCheckpoint, TrainingRun};

/// Rollout caps a policy's action chunk; longer chunks drift from the scene.
const MAX_ROLLOUT_ACTION_STEPS: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::rollout) struct CheckpointChoice {
    identity: CheckpointIdentity,
    step: u64,
    validation_loss: Option<f64>,
    policy: CheckpointPolicy,
}

#[derive(Clone, Debug, PartialEq)]
struct CheckpointIdentity {
    key: String,
    run_id: String,
    run_name: String,
    run_active: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CheckpointPolicy {
    action_steps: usize,
    sample_hz: Option<f64>,
    camera_roles: Vec<String>,
}

impl CheckpointChoice {
    /// Every checkpoint whose run Studio still records, newest step first.
    pub(super) fn listed(runs: &[TrainingRun], checkpoints: &[TrainingCheckpoint]) -> Vec<Self> {
        let mut choices = checkpoints
            .iter()
            .filter_map(|checkpoint| {
                let run = runs.iter().find(|run| run.id() == checkpoint.run_id())?;
                Some(Self {
                    identity: CheckpointIdentity {
                        key: format!("{}::{}", run.id(), checkpoint.step()),
                        run_id: run.id().to_owned(),
                        run_name: run.config().run_name().to_owned(),
                        run_active: run.phase().active(),
                    },
                    step: checkpoint.step(),
                    validation_loss: checkpoint.validation_loss(),
                    policy: CheckpointPolicy::from(checkpoint),
                })
            })
            .collect::<Vec<_>>();
        choices.sort_by_key(|choice| std::cmp::Reverse(choice.step));
        choices
    }

    /// The key the checkpoint picker stores.
    pub(super) fn key(&self) -> &str {
        &self.identity.key
    }

    pub(super) fn run_id(&self) -> &str {
        &self.identity.run_id
    }

    pub(super) const fn step(&self) -> u64 {
        self.step
    }

    /// The checkpoint as the picker lists it.
    pub(super) fn label(&self) -> String {
        use crate::format::GroupedCountPresentation as _;
        let training = if self.identity.run_active {
            " · training"
        } else {
            ""
        };
        format!(
            "{} · step {}{training}",
            self.identity.run_name,
            self.step.grouped_label()
        )
    }

    /// Whether its run is still training, which a rollout must wait out.
    pub(super) const fn run_active(&self) -> bool {
        self.identity.run_active
    }

    pub(super) const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }

    pub(super) const fn action_steps(&self) -> usize {
        self.policy.action_steps
    }

    pub(super) const fn sample_hz(&self) -> Option<f64> {
        self.policy.sample_hz
    }

    /// The camera roles the policy was trained on, in first-seen order.
    pub(super) fn camera_roles(&self) -> &[String] {
        &self.policy.camera_roles
    }
}

impl From<&TrainingCheckpoint> for CheckpointPolicy {
    fn from(checkpoint: &TrainingCheckpoint) -> Self {
        let policy = checkpoint.policy();
        // A recorded camera stream is named `{robot}.camera.{role}`; the policy
        // was trained on the roles, in first-seen order.
        let mut camera_roles = Vec::new();
        for stream in policy.camera_streams() {
            let role = stream.rsplit('.').next().unwrap_or(stream).to_owned();
            if !camera_roles.contains(&role) {
                camera_roles.push(role);
            }
        }
        Self {
            action_steps: policy.action_steps().min(MAX_ROLLOUT_ACTION_STEPS),
            sample_hz: policy.sample_hz(),
            camera_roles,
        }
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::training::{
        CheckpointPolicy as TrainedPolicy, TrainingCheckpoint, TrainingConfig,
        TrainingInputSummary, TrainingRun, TrainingRunIdentity, TrainingRunPhase,
        TrainingRunStatus,
    };

    use super::{CheckpointChoice, MAX_ROLLOUT_ACTION_STEPS};

    /// A recorded camera stream is `{robot}.camera.{role}`; the policy names
    /// the roles, once each.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn checkpoints_of_recorded_runs_are_listed_newest_first() {
        let run = TrainingRun::new(
            TrainingRunIdentity::new(
                "active-run",
                TrainingConfig::default(),
                TrainingInputSummary::default(),
                1,
            ),
            TrainingRunStatus::new(TrainingRunPhase::Running, 0, None),
        );
        let policy = TrainedPolicy::new(
            vec![
                "follower.camera.gripper".to_owned(),
                "follower.camera.top".to_owned(),
                "leader.camera.top".to_owned(),
            ],
            100,
            Some(30.0),
        );
        let checkpoints = [
            TrainingCheckpoint::new("active-run", 10_000).with_policy(policy.clone()),
            TrainingCheckpoint::new("active-run", 20_000).with_policy(policy),
            TrainingCheckpoint::new("deleted-run", 30_000),
        ];

        let choices = CheckpointChoice::listed(&[run], &checkpoints);

        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].step(), 20_000);
        assert_eq!(choices[0].camera_roles(), ["gripper", "top"]);
        assert!(choices[0].run_active());
        assert_eq!(choices[0].action_steps(), MAX_ROLLOUT_ACTION_STEPS);
    }
}
