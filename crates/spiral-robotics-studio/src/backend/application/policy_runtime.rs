//! Policy artifact and live-control invariants.
//!
//! SafeTensors, JSON, camera receivers, inference runtimes, and motor actors
//! map their observations into these plain models at adapter boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use spiral_robotics_contracts::api::MAX_ROLLOUT_ACTION_STEPS;

use super::error::ApplicationError;

pub(crate) const ACT_ARTIFACT_FORMAT: &str = "spiral_robotics.act.safetensors.v2";
pub(crate) const ACT_TENSOR_NAMESPACE: &str = "burn_act_state_dict_v1";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArtifactIdentity {
    pub format: String,
    pub purpose: String,
    pub run_id: String,
    pub step: u64,
    pub tensor_namespace: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PolicyDataContract {
    pub value_space: String,
    pub robot_type: String,
    pub joint_names: Vec<String>,
    pub camera_streams: Vec<String>,
    pub sample_hz: f64,
    pub image_layout: String,
    pub image_normalization: String,
    pub frame_alignment: String,
    pub schema_version: u32,
    pub supported_schema_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PolicyModelContract {
    pub state_dimensions: usize,
    pub action_dimensions: usize,
    pub action_chunk_size: usize,
    pub recommended_action_steps: usize,
    pub image_streams: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetRobotContract {
    pub id: String,
    pub robot_type: Option<String>,
    pub joint_names: Vec<String>,
    pub camera_bindings: Vec<CameraBindingContract>,
    pub connected: bool,
    pub calibrated: bool,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CameraBindingContract {
    pub role: String,
    pub camera_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ValidatedRolloutContract {
    pub joint_names: Vec<String>,
    pub camera_bindings: Vec<ResolvedCameraBinding>,
    pub sample_hz: f64,
    pub sample_period: Duration,
    pub action_steps: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedCameraBinding {
    pub stream: String,
    pub camera_id: String,
}

pub(crate) fn validate_artifact_identity(
    actual: &ArtifactIdentity,
    expected_run: &str,
    expected_step: u64,
) -> Result<(), ApplicationError> {
    if actual.format != ACT_ARTIFACT_FORMAT
        || actual.purpose != "inference"
        || actual.tensor_namespace != ACT_TENSOR_NAMESPACE
    {
        return invalid("checkpoint is not a native Spiral ACT inference artifact");
    }
    if actual.run_id != expected_run || actual.step != expected_step {
        return invalid("checkpoint manifest identity does not match the requested run");
    }
    Ok(())
}

/// Bind persisted artifact policy, loaded-model shape, and live robot setup.
pub(crate) fn validate_rollout_contract(
    data: PolicyDataContract,
    model: &PolicyModelContract,
    target: &TargetRobotContract,
) -> Result<ValidatedRolloutContract, ApplicationError> {
    if data.robot_type.trim().is_empty()
        || data.joint_names.is_empty()
        || data.camera_streams.is_empty()
    {
        return invalid("checkpoint has an empty robot, joint, or camera contract");
    }
    if data.value_space != "centered_normalized"
        || data.image_layout != "chw"
        || data.image_normalization != "imagenet"
        || data.frame_alignment != "fixed_fps"
    {
        return invalid("checkpoint uses an unsupported observation or image contract");
    }
    if data.schema_version != data.supported_schema_version {
        return invalid(format!(
            "checkpoint schema version {} is not supported by runtime version {}",
            data.schema_version, data.supported_schema_version
        ));
    }
    require_unique_nonempty("checkpoint joint", &data.joint_names)?;
    require_unique_nonempty("checkpoint camera", &data.camera_streams)?;
    if !data.sample_hz.is_finite() || !(1.0..=100.0).contains(&data.sample_hz) {
        return invalid("checkpoint sample_hz must be between 1 and 100");
    }
    if model.state_dimensions != data.joint_names.len()
        || model.action_dimensions != data.joint_names.len()
    {
        return invalid(format!(
            "checkpoint joint contract has {} names but the model expects {} state and {} action values",
            data.joint_names.len(),
            model.state_dimensions,
            model.action_dimensions
        ));
    }
    if model.recommended_action_steps == 0
        || model.recommended_action_steps > model.action_chunk_size
    {
        return invalid(format!(
            "checkpoint recommends {} action steps for chunk size {}",
            model.recommended_action_steps, model.action_chunk_size
        ));
    }
    if model.image_streams != data.camera_streams {
        return invalid("checkpoint camera order does not match its configuration");
    }
    if target.problem.is_some() || !target.connected || !target.calibrated {
        return invalid(format!(
            "robot {:?} must be valid, connected, and calibrated before rollout",
            target.id
        ));
    }
    if target.robot_type.as_deref() != Some(data.robot_type.as_str()) {
        return invalid(format!(
            "checkpoint was trained for {:?}, but robot {:?} is {:?}",
            data.robot_type, target.id, target.robot_type
        ));
    }
    let configured = target.joint_names.iter().collect::<BTreeSet<_>>();
    let expected = data.joint_names.iter().collect::<BTreeSet<_>>();
    if configured != expected || target.joint_names.len() != data.joint_names.len() {
        return invalid("checkpoint joint names do not exactly match the target robot");
    }

    let mut bindings = BTreeMap::new();
    for binding in &target.camera_bindings {
        if binding.role.trim().is_empty()
            || binding.role.trim() != binding.role
            || binding.camera_id.trim().is_empty()
            || bindings
                .insert(binding.role.as_str(), binding.camera_id.as_str())
                .is_some()
        {
            return invalid("robot camera roles must be non-empty and unique");
        }
    }
    let mut resolved = Vec::with_capacity(data.camera_streams.len());
    for stream in &data.camera_streams {
        let role = stream.rsplit('.').next().unwrap_or(stream);
        let camera_id = bindings.get(role).ok_or_else(|| {
            ApplicationError::InvalidInput(format!(
                "robot {:?} has no camera binding for checkpoint stream {stream:?}",
                target.id
            ))
        })?;
        resolved.push(ResolvedCameraBinding {
            stream: stream.clone(),
            camera_id: (*camera_id).to_owned(),
        });
    }

    Ok(ValidatedRolloutContract {
        joint_names: data.joint_names,
        camera_bindings: resolved,
        sample_hz: data.sample_hz,
        sample_period: Duration::from_secs_f64(1.0 / data.sample_hz),
        action_steps: model.recommended_action_steps.min(MAX_ROLLOUT_ACTION_STEPS),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObservationFreshness {
    pub source: String,
    pub age: Option<Duration>,
}

pub(crate) fn require_fresh_observations(
    observations: &[ObservationFreshness],
    maximum_age: Duration,
) -> Result<(), ApplicationError> {
    for observation in observations {
        let age = observation.age.ok_or_else(|| {
            ApplicationError::Conflict(format!("camera {:?} has no frame", observation.source))
        })?;
        if age > maximum_age {
            return Err(ApplicationError::Conflict(format!(
                "camera {:?} became stale",
                observation.source
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreparedActionGoals {
    pub goals: BTreeMap<String, f64>,
    pub clamps: Vec<ActionClamp>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActionClamp {
    pub index: usize,
    pub joint: String,
    pub requested: f32,
    pub applied: f32,
}

/// Bind one model action to the exact ordered joint contract before hardware.
///
/// Finite values outside the normalized range are safe to saturate at the
/// calibrated endpoint. Shape mismatches and non-finite values remain fatal.
pub(crate) fn prepare_action_goals(
    joint_names: &[String],
    action: &[f32],
) -> Result<PreparedActionGoals, ApplicationError> {
    if action.len() != joint_names.len() {
        return invalid(format!(
            "policy returned {} action values for {} joints",
            action.len(),
            joint_names.len()
        ));
    }
    let mut goals = BTreeMap::new();
    let mut clamps = Vec::new();
    for (index, (joint, value)) in joint_names.iter().zip(action).enumerate() {
        if !value.is_finite() {
            return Err(ApplicationError::Conflict(format!(
                "policy returned a non-finite action at index {index} for joint {joint:?}"
            )));
        }
        let applied = value.clamp(-1.0, 1.0);
        if applied != *value {
            clamps.push(ActionClamp {
                index,
                joint: joint.clone(),
                requested: *value,
                applied,
            });
        }
        goals.insert(joint.clone(), f64::from(applied));
    }
    Ok(PreparedActionGoals { goals, clamps })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JointObservation {
    pub joint: String,
    pub normalized: Option<f64>,
}

/// Order live telemetry exactly as the model contract declares it.
pub(crate) fn prepare_observation_state(
    joint_names: &[String],
    observations: &[JointObservation],
) -> Result<Vec<f32>, ApplicationError> {
    let mut values = BTreeMap::new();
    for observation in observations {
        if values
            .insert(observation.joint.as_str(), observation.normalized)
            .is_some()
        {
            return Err(ApplicationError::Conflict(format!(
                "joint {:?} appeared more than once in live telemetry",
                observation.joint
            )));
        }
    }
    joint_names
        .iter()
        .map(|joint| {
            values
                .get(joint.as_str())
                .copied()
                .flatten()
                .filter(|value| value.is_finite())
                .map(|value| value as f32)
                .ok_or_else(|| {
                    ApplicationError::Conflict(format!(
                        "joint {joint:?} has no finite calibrated telemetry"
                    ))
                })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlOwner {
    Policy { robot: String, session: String },
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PolicyOwnership {
    Retained,
    Replaced(String),
}

pub(crate) fn assess_policy_ownership(
    owner: &ControlOwner,
    hardware_error: Option<&str>,
    expected_robot: &str,
    expected_session: &str,
) -> Result<PolicyOwnership, ApplicationError> {
    if matches!(owner, ControlOwner::Policy { robot, session }
        if robot == expected_robot && session == expected_session)
    {
        return Ok(PolicyOwnership::Retained);
    }
    let replacement = match owner {
        ControlOwner::Policy { robot, session } => {
            format!("Policy session {session} on {robot}")
        }
        ControlOwner::Other(description) => description.clone(),
    };
    if let Some(error) = hardware_error {
        return Err(ApplicationError::Conflict(format!(
            "stopped policy: hardware control transitioned to {replacement}: {error}"
        )));
    }
    Ok(PolicyOwnership::Replaced(format!(
        "Policy stopped; hardware control transitioned to {replacement}."
    )))
}

fn require_unique_nonempty(label: &str, values: &[String]) -> Result<(), ApplicationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() || value.trim() != value || !seen.insert(value) {
            return invalid(format!("{label} names must be non-empty and unique"));
        }
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ApplicationError> {
    Err(ApplicationError::InvalidInput(message.into()))
}

#[cfg(test)]
#[path = "policy_runtime/tests.rs"]
mod tests;
