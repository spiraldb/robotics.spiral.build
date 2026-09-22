//! Policy artifact and live-control invariants.
//!
//! SafeTensors, JSON, camera receivers, inference runtimes, and motor actors
//! map their observations into these plain models at adapter boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::OperationError;

pub(crate) const ACT_ARTIFACT_FORMAT: &str = "spiral_robotics.act.safetensors.v2";
pub(crate) const ACT_TENSOR_NAMESPACE: &str = "lerobot_act_state_dict_v1";
const LEGACY_BURN_ACT_TENSOR_NAMESPACE: &str = "burn_act_state_dict_v1";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArtifactIdentity {
    format: String,
    purpose: String,
    run_id: String,
    step: u64,
    tensor_namespace: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PolicyDataContract {
    observations: PolicyObservationContract,
    images: PolicyImageContract,
    schemas: PolicySchemaContract,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PolicyObservationContract {
    value_space: String,
    robot_type: String,
    joint_names: Vec<String>,
    camera_streams: Vec<String>,
    sample_hz: f64,
}

impl PolicyObservationContract {
    pub(super) fn new(
        value_space: impl Into<String>,
        robot_type: impl Into<String>,
        joint_names: Vec<String>,
    ) -> Self {
        Self {
            value_space: value_space.into(),
            robot_type: robot_type.into(),
            joint_names,
            camera_streams: Vec::new(),
            sample_hz: 0.0,
        }
    }

    pub(super) fn with_capture(mut self, camera_streams: Vec<String>, sample_hz: f64) -> Self {
        self.camera_streams = camera_streams;
        self.sample_hz = sample_hz;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PolicyImageContract {
    layout: String,
    normalization: String,
    frame_alignment: String,
}

impl PolicyImageContract {
    pub(super) fn new(
        layout: impl Into<String>,
        normalization: impl Into<String>,
        frame_alignment: impl Into<String>,
    ) -> Self {
        Self {
            layout: layout.into(),
            normalization: normalization.into(),
            frame_alignment: frame_alignment.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PolicySchemaContract {
    artifact: u32,
    supported: u32,
}

impl PolicySchemaContract {
    pub(super) fn new(artifact: u32, supported: u32) -> Self {
        Self {
            artifact,
            supported,
        }
    }
}

impl PolicyDataContract {
    pub(super) fn new(
        observations: PolicyObservationContract,
        images: PolicyImageContract,
        schemas: PolicySchemaContract,
    ) -> Self {
        Self {
            observations,
            images,
            schemas,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PolicyModelContract {
    state_dimensions: usize,
    action_dimensions: usize,
    action_chunk_size: usize,
    recommended_action_steps: usize,
    image_streams: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TargetRobotContract {
    id: String,
    robot_type: Option<String>,
    joint_names: Vec<String>,
    camera_bindings: Vec<CameraBindingContract>,
    connected: bool,
    calibrated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CameraBindingContract {
    role: String,
    camera_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ValidatedRolloutContract {
    joint_names: Vec<String>,
    camera_bindings: Vec<ResolvedCameraBinding>,
    sample_hz: f64,
    sample_period: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedCameraBinding {
    stream: String,
    camera_id: String,
}

impl ArtifactIdentity {
    pub(super) fn new(
        format: impl Into<String>,
        purpose: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            format: format.into(),
            purpose: purpose.into(),
            run_id: run_id.into(),
            step: 0,
            tensor_namespace: String::new(),
        }
    }
    pub(super) fn with_artifact(mut self, step: u64, tensor_namespace: impl Into<String>) -> Self {
        self.step = step;
        self.tensor_namespace = tensor_namespace.into();
        self
    }

    pub(super) fn validate_for(
        &self,
        expected_run: &str,
        expected_step: u64,
    ) -> Result<(), OperationError> {
        let supported_namespace = matches!(
            self.tensor_namespace.as_str(),
            ACT_TENSOR_NAMESPACE | LEGACY_BURN_ACT_TENSOR_NAMESPACE
        );
        if self.format != ACT_ARTIFACT_FORMAT || self.purpose != "inference" || !supported_namespace
        {
            return Err(OperationError::invalid_input(
                "checkpoint is not a native Spiral ACT inference artifact",
            ));
        }
        if self.run_id != expected_run || self.step != expected_step {
            return Err(OperationError::invalid_input(
                "checkpoint manifest identity does not match the requested run",
            ));
        }
        Ok(())
    }
}

impl PolicyModelContract {
    pub(super) fn new(
        state_dimensions: usize,
        action_dimensions: usize,
        image_streams: Vec<String>,
    ) -> Self {
        Self {
            state_dimensions,
            action_dimensions,
            action_chunk_size: 0,
            recommended_action_steps: 0,
            image_streams,
        }
    }
    pub(super) fn with_action_shape(
        mut self,
        action_chunk_size: usize,
        recommended_action_steps: usize,
    ) -> Self {
        self.action_chunk_size = action_chunk_size;
        self.recommended_action_steps = recommended_action_steps;
        self
    }
}

impl TargetRobotContract {
    pub(super) fn new(
        id: impl Into<String>,
        robot_type: Option<String>,
        joint_names: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            robot_type,
            joint_names,
            camera_bindings: Vec::new(),
            connected: false,
            calibrated: false,
        }
    }
    pub(super) fn with_cameras(mut self, camera_bindings: Vec<CameraBindingContract>) -> Self {
        self.camera_bindings = camera_bindings;
        self
    }
    pub(super) fn with_availability(mut self, connected: bool, calibrated: bool) -> Self {
        self.connected = connected;
        self.calibrated = calibrated;
        self
    }
}

impl CameraBindingContract {
    pub(super) fn new(role: impl Into<String>, camera_id: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            camera_id: camera_id.into(),
        }
    }
}

impl ValidatedRolloutContract {
    pub(super) fn joint_names(&self) -> &[String] {
        &self.joint_names
    }
    pub(super) fn camera_bindings(&self) -> &[ResolvedCameraBinding] {
        &self.camera_bindings
    }
    pub(super) const fn sample_hz(&self) -> f64 {
        self.sample_hz
    }
    pub(super) const fn sample_period(&self) -> Duration {
        self.sample_period
    }
}

impl ResolvedCameraBinding {
    pub(super) fn stream(&self) -> &str {
        &self.stream
    }
    pub(super) fn camera_id(&self) -> &str {
        &self.camera_id
    }
}

/// Bind persisted artifact policy, loaded-model shape, and live robot setup.
impl PolicyDataContract {
    pub(super) fn validated_with(
        self,
        model: &PolicyModelContract,
        target: &TargetRobotContract,
    ) -> Result<ValidatedRolloutContract, OperationError> {
        let data = self;
        if data.observations.robot_type.trim().is_empty()
            || data.observations.joint_names.is_empty()
            || data.observations.camera_streams.is_empty()
        {
            return Err(OperationError::invalid_input(
                "checkpoint has an empty robot, joint, or camera contract",
            ));
        }
        if data.observations.value_space != "centered_normalized"
            || data.images.layout != "chw"
            || data.images.normalization != "imagenet"
            || data.images.frame_alignment != "fixed_fps"
        {
            return Err(OperationError::invalid_input(
                "checkpoint uses an unsupported observation or image contract",
            ));
        }
        if data.schemas.artifact != data.schemas.supported {
            return Err(OperationError::invalid_input(format!(
                "checkpoint schema version {} is not supported by runtime version {}",
                data.schemas.artifact, data.schemas.supported
            )));
        }
        data.observations
            .joint_names
            .require_unique_nonempty("checkpoint joint")?;
        data.observations
            .camera_streams
            .require_unique_nonempty("checkpoint camera")?;
        if !data.observations.sample_hz.is_finite()
            || !(1.0..=100.0).contains(&data.observations.sample_hz)
        {
            return Err(OperationError::invalid_input(
                "checkpoint sample_hz must be between 1 and 100",
            ));
        }
        if model.state_dimensions != data.observations.joint_names.len()
            || model.action_dimensions != data.observations.joint_names.len()
        {
            return Err(OperationError::invalid_input(format!(
                "checkpoint joint contract has {} names but the model expects {} state and {} action values",
                data.observations.joint_names.len(),
                model.state_dimensions,
                model.action_dimensions
            )));
        }
        if model.recommended_action_steps == 0
            || model.recommended_action_steps > model.action_chunk_size
        {
            return Err(OperationError::invalid_input(format!(
                "checkpoint recommends {} action steps for chunk size {}",
                model.recommended_action_steps, model.action_chunk_size
            )));
        }
        if model.image_streams != data.observations.camera_streams {
            return Err(OperationError::invalid_input(
                "checkpoint camera order does not match its configuration",
            ));
        }
        if !target.connected || !target.calibrated {
            return Err(OperationError::invalid_input(format!(
                "robot {:?} must be connected and calibrated before rollout",
                target.id
            )));
        }
        if target.robot_type.as_deref() != Some(data.observations.robot_type.as_str()) {
            return Err(OperationError::invalid_input(format!(
                "checkpoint was trained for {:?}, but robot {:?} is {:?}",
                data.observations.robot_type, target.id, target.robot_type
            )));
        }
        let configured = target.joint_names.iter().collect::<BTreeSet<_>>();
        let expected = data
            .observations
            .joint_names
            .iter()
            .collect::<BTreeSet<_>>();
        if configured != expected || target.joint_names.len() != data.observations.joint_names.len()
        {
            return Err(OperationError::invalid_input(
                "checkpoint joint names do not exactly match the target robot",
            ));
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
                return Err(OperationError::invalid_input(
                    "robot camera roles must be non-empty and unique",
                ));
            }
        }
        let mut resolved = Vec::with_capacity(data.observations.camera_streams.len());
        for stream in &data.observations.camera_streams {
            let role = stream.rsplit('.').next().unwrap_or(stream);
            let camera_id = bindings.get(role).ok_or_else(|| {
                OperationError::InvalidInput(format!(
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
            joint_names: data.observations.joint_names,
            camera_bindings: resolved,
            sample_hz: data.observations.sample_hz,
            sample_period: Duration::from_secs_f64(1.0 / data.observations.sample_hz),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObservationFreshness {
    source: String,
    age: Option<Duration>,
}

impl ObservationFreshness {
    pub(super) fn new(source: impl Into<String>, age: Option<Duration>) -> Self {
        Self {
            source: source.into(),
            age,
        }
    }
}

pub(crate) trait ObservationFreshnessCollection {
    fn require_fresh(&self, maximum_age: Duration) -> Result<(), OperationError>;
}

impl ObservationFreshnessCollection for [ObservationFreshness] {
    fn require_fresh(&self, maximum_age: Duration) -> Result<(), OperationError> {
        for observation in self {
            let age = observation.age.ok_or_else(|| {
                OperationError::Conflict(format!("camera {:?} has no frame", observation.source))
            })?;
            if age > maximum_age {
                return Err(OperationError::Conflict(format!(
                    "camera {:?} became stale",
                    observation.source
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreparedActionGoals {
    goals: BTreeMap<String, f64>,
    clamps: Vec<ActionClamp>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActionClamp {
    index: usize,
    joint: String,
    requested: f32,
    applied: f32,
}

impl PreparedActionGoals {
    pub(super) fn goals(&self) -> &BTreeMap<String, f64> {
        &self.goals
    }
    pub(super) fn clamps(&self) -> &[ActionClamp] {
        &self.clamps
    }

    /// Bind one model action to the exact ordered joint contract before
    /// hardware control.
    pub(super) fn from_action(
        joint_names: &[String],
        action: &[f32],
    ) -> Result<Self, OperationError> {
        if action.len() != joint_names.len() {
            return Err(OperationError::invalid_input(format!(
                "policy returned {} action values for {} joints",
                action.len(),
                joint_names.len()
            )));
        }
        let mut goals = BTreeMap::new();
        let mut clamps = Vec::new();
        for (index, (joint, value)) in joint_names.iter().zip(action).enumerate() {
            if !value.is_finite() {
                return Err(OperationError::Conflict(format!(
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
        Ok(Self { goals, clamps })
    }
}

impl ActionClamp {
    pub(super) const fn index(&self) -> usize {
        self.index
    }
    pub(super) fn joint(&self) -> &str {
        &self.joint
    }
    pub(super) const fn requested(&self) -> f32 {
        self.requested
    }
    pub(super) const fn applied(&self) -> f32 {
        self.applied
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JointObservation {
    joint: String,
    normalized: Option<f64>,
}

impl JointObservation {
    pub(super) fn new(joint: impl Into<String>, normalized: Option<f64>) -> Self {
        Self {
            joint: joint.into(),
            normalized,
        }
    }
}

pub(crate) trait JointObservations {
    fn ordered_state(&self, joint_names: &[String]) -> Result<Vec<f32>, OperationError>;
}

impl JointObservations for [JointObservation] {
    /// Order live telemetry exactly as the model contract declares it.
    fn ordered_state(&self, joint_names: &[String]) -> Result<Vec<f32>, OperationError> {
        let mut values = BTreeMap::new();
        for observation in self {
            if values
                .insert(observation.joint.as_str(), observation.normalized)
                .is_some()
            {
                return Err(OperationError::Conflict(format!(
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
                        OperationError::Conflict(format!(
                            "joint {joint:?} has no finite calibrated telemetry"
                        ))
                    })
            })
            .collect()
    }
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

pub(crate) struct ExpectedPolicyOwner<'a> {
    robot: &'a str,
    session: &'a str,
}

impl<'a> ExpectedPolicyOwner<'a> {
    pub(super) fn new(robot: &'a str, session: &'a str) -> Self {
        Self { robot, session }
    }

    pub(super) fn robot(&self) -> &str {
        self.robot
    }

    pub(super) fn session(&self) -> &str {
        self.session
    }
}

impl ControlOwner {
    pub(super) fn assess(
        &self,
        hardware_error: Option<&str>,
        expected: ExpectedPolicyOwner<'_>,
    ) -> Result<PolicyOwnership, OperationError> {
        if matches!(self, ControlOwner::Policy { robot, session }
            if robot == expected.robot && session == expected.session)
        {
            return Ok(PolicyOwnership::Retained);
        }
        let replacement = match self {
            ControlOwner::Policy { robot, session } => {
                format!("Policy session {session} on {robot}")
            }
            ControlOwner::Other(description) => description.clone(),
        };
        if let Some(error) = hardware_error {
            return Err(OperationError::Conflict(format!(
                "stopped policy: hardware control transitioned to {replacement}: {error}"
            )));
        }
        Ok(PolicyOwnership::Replaced(format!(
            "Policy stopped; hardware control transitioned to {replacement}."
        )))
    }
}

trait UniqueNames {
    fn require_unique_nonempty(&self, label: &str) -> Result<(), OperationError>;
}

impl UniqueNames for [String] {
    fn require_unique_nonempty(&self, label: &str) -> Result<(), OperationError> {
        let mut seen = BTreeSet::new();
        for value in self {
            if value.trim().is_empty() || value.trim() != value || !seen.insert(value) {
                return Err(OperationError::invalid_input(format!(
                    "{label} names must be non-empty and unique"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "policy/tests.rs"]
mod tests;
