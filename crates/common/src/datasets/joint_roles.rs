//! Which recorded streams hold a dataset's joints, arm by arm: what the
//! dataset checks read to measure how the robot moved. Unlike a robot model,
//! roles need no robot description, so a dataset without one still gets its
//! joints checked.
use super::{DatasetRobot, DatasetSelection, JointBinding, JointSelector};
use serde::{Deserialize, Serialize};

/// The joint roles the browser resolved for a dataset's current version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetJointRolesSave {
    dataset: DatasetSelection,
    roles: DatasetJointRoles,
}
impl DatasetJointRolesSave {
    pub fn new(dataset: DatasetSelection, roles: DatasetJointRoles) -> Self {
        Self { dataset, roles }
    }
    pub fn into_parts(self) -> (DatasetSelection, DatasetJointRoles) {
        (self.dataset, self.roles)
    }
}

/// Every arm a dataset records; none when no stream records joint positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetJointRoles {
    arms: Vec<ArmRoles>,
}

impl DatasetJointRoles {
    pub fn arms(&self) -> &[ArmRoles] {
        &self.arms
    }
    /// Check the shape every role must have; whether the named streams exist
    /// is for whoever holds their messages to check.
    pub fn validate(&self) -> Result<(), String> {
        if self.arms.len() > 8 {
            return Err("A dataset records at most eight arms".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for arm in &self.arms {
            if arm.name.trim().is_empty() || !names.insert(arm.name.as_str()) {
                return Err("Every arm needs a distinct name".into());
            }
            arm.state.validate()?;
            if arm.state.indices.is_empty() {
                return Err(format!("The {} arm's state names no joints", arm.name));
            }
            if let Some(action) = &arm.action {
                action.validate()?;
                if action.indices.len() != arm.state.indices.len() {
                    return Err(format!(
                        "The {} arm's action must command the same joints as its state",
                        arm.name
                    ));
                }
            }
            if let Some(gripper) = &arm.gripper {
                gripper.validate(&arm.name)?;
            }
        }
        Ok(())
    }
}

impl DatasetJointRoles {
    /// What a policy trained on these roles reads and commands: arm by arm,
    /// the arm's joints and then its gripper's opening — measured for the
    /// state, commanded for the action. An arm without commanded positions
    /// leaves nothing to learn.
    pub fn policy_joints(&self) -> Result<PolicyJoints<'_>, String> {
        if self.arms.is_empty() {
            return Err("These joint roles name no arm.".into());
        }
        let mut joints = PolicyJoints {
            state: Vec::new(),
            action: Vec::new(),
        };
        for arm in &self.arms {
            let commanded = arm.action.as_ref().ok_or_else(|| {
                format!("The {} arm records no commanded positions.", arm.name)
            })?;
            let names = (1..=arm.state.indices.len().max(1))
                .map(|joint| format!("{}.j{joint}", arm.name))
                .collect::<Vec<_>>();
            joints.state.push(NamedValues::new(&arm.state, names.clone()));
            joints.action.push(NamedValues::new(commanded, names));
            if let Some(gripper) = &arm.gripper {
                let name = vec![format!("{}.gripper", arm.name)];
                joints.state.push(NamedValues::new(&gripper.state, name.clone()));
                joints.action.push(NamedValues::new(&gripper.action, name));
            }
        }
        Ok(joints)
    }
}

/// The values a policy's state and its action concatenate, in order.
pub struct PolicyJoints<'a> {
    state: Vec<NamedValues<'a>>,
    action: Vec<NamedValues<'a>>,
}

impl<'a> PolicyJoints<'a> {
    pub fn state(&self) -> &[NamedValues<'a>] {
        &self.state
    }
    pub fn action(&self) -> &[NamedValues<'a>] {
        &self.action
    }
    /// What each state value is: `left.j1`, …, `left.gripper`.
    pub fn state_names(&self) -> Vec<String> {
        Self::names(&self.state)
    }
    /// What each action value is, in the state's order.
    pub fn action_names(&self) -> Vec<String> {
        Self::names(&self.action)
    }
    fn names(values: &[NamedValues<'a>]) -> Vec<String> {
        values
            .iter()
            .flat_map(|values| values.names.iter().cloned())
            .collect()
    }
}

/// Numbers recorded in one stream, and what each one is.
pub struct NamedValues<'a> {
    values: &'a RecordedValues,
    names: Vec<String>,
}

impl<'a> NamedValues<'a> {
    fn new(values: &'a RecordedValues, names: Vec<String>) -> Self {
        Self { values, names }
    }
    pub fn values(&self) -> &'a RecordedValues {
        self.values
    }
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// One arm: its joints' measured and commanded positions, and its gripper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArmRoles {
    /// What an operator calls the arm: "left", "right", or the robot's name
    /// for a lone arm.
    name: String,
    /// Measured positions of the arm's joints, the gripper excluded.
    state: RecordedValues,
    /// Commanded positions of the same joints, in the same order.
    action: Option<RecordedValues>,
    gripper: Option<GripperRoles>,
}

impl ArmRoles {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn state(&self) -> &RecordedValues {
        &self.state
    }
    pub fn action(&self) -> Option<&RecordedValues> {
        self.action.as_ref()
    }
    pub fn gripper(&self) -> Option<&GripperRoles> {
        self.gripper.as_ref()
    }
}

/// A gripper's measured and commanded opening, and the recorded values of a
/// fully open and a fully closed gripper, so every dataset's gripper reads
/// on one scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GripperRoles {
    /// The measured opening: one index, or none for a number field.
    state: RecordedValues,
    /// The commanded opening, in the same units.
    action: RecordedValues,
    open: f64,
    closed: f64,
}

impl GripperRoles {
    pub fn state(&self) -> &RecordedValues {
        &self.state
    }
    pub fn action(&self) -> &RecordedValues {
        &self.action
    }
    /// The recorded values of a fully open and a fully closed gripper.
    pub fn range(&self) -> (f64, f64) {
        (self.open, self.closed)
    }
    fn validate(&self, arm: &str) -> Result<(), String> {
        self.state.validate()?;
        self.action.validate()?;
        if self.state.indices.len() > 1 || self.action.indices.len() > 1 {
            return Err(format!("The {arm} gripper is one number, not several"));
        }
        if !self.open.is_finite() || !self.closed.is_finite() || self.open == self.closed {
            return Err(format!(
                "The {arm} gripper's open and closed values must differ"
            ));
        }
        Ok(())
    }
}

/// Numbers recorded in one field of one stream: positions `indices` of the
/// list at `field` — each item's number at `element` below it — or, with no
/// indices, the number at `field` itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordedValues {
    topic: String,
    schema: String,
    /// Structured path below the recorded message's value.
    field: Vec<String>,
    /// Path below each list item to its number; empty when items are numbers.
    element: Vec<String>,
    indices: Vec<usize>,
}

impl RecordedValues {
    pub fn topic(&self) -> &str {
        &self.topic
    }
    pub fn schema(&self) -> &str {
        &self.schema
    }
    pub fn field(&self) -> &[String] {
        &self.field
    }
    pub fn element(&self) -> &[String] {
        &self.element
    }
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
    /// The path a robot binding names these numbers by: the field, then
    /// each item's element.
    pub fn path(&self) -> Vec<String> {
        self.field.iter().chain(&self.element).cloned().collect()
    }
    /// The robot binding that names the number at list position `index` of
    /// these values: the same stream, the same path, the same position.
    pub fn binding<'a>(&self, robot: &'a DatasetRobot, index: usize) -> Option<&'a JointBinding> {
        let path = self.path();
        robot.bindings().iter().find(|binding| {
            binding.topic() == self.topic
                && binding.schema() == self.schema
                && binding.field() == path
                && binding.selector() == Some(&JointSelector::Index { index })
        })
    }
    /// Every number these roles name in one recorded message, in order;
    /// `None` when one of them is missing.
    pub fn read(&self, message: &serde_json::Value) -> Option<Vec<f64>> {
        let field = Self::walk(message, &self.field)?;
        if self.indices.is_empty() {
            return Some(vec![field.as_f64()?]);
        }
        self.indices
            .iter()
            .map(|index| Self::walk(field.as_array()?.get(*index)?, &self.element)?.as_f64())
            .collect()
    }
    fn walk<'a>(value: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
        path.iter().try_fold(value, |value, name| value.get(name))
    }
    fn validate(&self) -> Result<(), String> {
        let named = |path: &[String]| path.iter().all(|name| !name.is_empty());
        if self.topic.is_empty()
            || self.schema.is_empty()
            || self.field.is_empty()
            || !named(&self.field)
            || !named(&self.element)
            || (self.indices.is_empty() && !self.element.is_empty())
            || self.indices.len() > 64
        {
            return Err(format!("Invalid recorded field on {}", self.topic));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roles(value: serde_json::Value) -> DatasetJointRoles {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_policy_reads_each_arm_then_its_gripper_and_needs_commands() {
        let values = |topic: &str, indices: &[usize]| {
            json!({"topic": topic, "schema": "RobotState", "field": ["position"], "element": [], "indices": indices})
        };
        let gripper = json!({"state": values("/left-ee-state", &[0]), "action": values("/left-ee-action", &[0]),
            "open": 1.0, "closed": 0.0});
        let two_arms = roles(json!({"arms": [
            {"name": "left", "state": values("/left-arm-state", &[0, 1]),
             "action": values("/left-arm-action", &[0, 1]), "gripper": gripper},
            {"name": "right", "state": values("/right-arm-state", &[0]),
             "action": values("/right-arm-action", &[0]), "gripper": null},
        ]}));
        let joints = two_arms.policy_joints().unwrap();
        assert_eq!(
            joints.state_names(),
            ["left.j1", "left.j2", "left.gripper", "right.j1"]
        );
        assert_eq!(joints.action_names(), joints.state_names());
        assert_eq!(joints.action()[1].values().topic(), "/left-ee-action");
        let uncommanded = roles(json!({"arms": [
            {"name": "follower", "state": values("/joints", &[0]), "action": null, "gripper": null}
        ]}));
        assert_eq!(
            uncommanded.policy_joints().err().unwrap(),
            "The follower arm records no commanded positions."
        );
    }

    #[test]
    fn lists_of_wrapped_numbers_and_plain_numbers_both_read_in_order() {
        let wrapped: RecordedValues = serde_json::from_value(json!({
            "topic": "/joints", "schema": "JointSample", "field": ["state_position", "values"],
            "element": ["value"], "indices": [2, 0]
        }))
        .unwrap();
        let message = json!({"state_position": {"values": [{"value": 0.5}, {"value": 0.25}, {"value": -1.0}]}});
        assert_eq!(wrapped.read(&message), Some(vec![-1.0, 0.5]));
        assert_eq!(wrapped.path(), ["state_position", "values", "value"]);
        assert_eq!(wrapped.read(&json!({"state_position": null})), None);
        let scalar: RecordedValues = serde_json::from_value(json!({
            "topic": "/gripper", "schema": "Gripper", "field": ["opening"], "element": [], "indices": []
        }))
        .unwrap();
        assert_eq!(scalar.read(&json!({"opening": 0.75})), Some(vec![0.75]));
    }

    #[test]
    fn a_robot_binding_names_the_same_stream_path_and_position() {
        let robot: DatasetRobot = serde_json::from_value(json!({
            "urdf": "<robot name=\"arm\"/>",
            "bindings": [{"joint": "elbow", "topic": "/joints", "schema": "JointSample",
                "field": ["state_position", "values", "value"], "selector": {"kind": "index", "index": 2},
                "scale": 1.0, "offset": 0.0}],
        }))
        .unwrap();
        let values: RecordedValues = serde_json::from_value(json!({
            "topic": "/joints", "schema": "JointSample", "field": ["state_position", "values"],
            "element": ["value"], "indices": [1, 2]
        }))
        .unwrap();
        assert_eq!(
            values.binding(&robot, 2).map(JointBinding::joint),
            Some("elbow")
        );
        assert!(values.binding(&robot, 1).is_none());
    }

    #[test]
    fn an_arm_commands_the_joints_it_measures_and_a_gripper_has_a_direction() {
        let values = |indices: serde_json::Value| json!({"topic": "/frame", "schema": "Frame", "field": ["state"], "element": [], "indices": indices});
        let arm = |action: serde_json::Value, gripper: serde_json::Value| {
            roles(
                json!({"arms": [{"name": "left", "state": values(json!([0, 1])), "action": action, "gripper": gripper}]}),
            )
        };
        assert!(arm(values(json!([7, 8])), json!(null)).validate().is_ok());
        assert!(arm(values(json!([7])), json!(null)).validate().is_err());
        let gripper = |open: f64| json!({"state": values(json!([6])), "action": values(json!([13])), "open": open, "closed": 0.0});
        assert!(arm(json!(null), gripper(1.0)).validate().is_ok());
        assert!(arm(json!(null), gripper(0.0)).validate().is_err());
        let twins = roles(json!({"arms": [
            {"name": "arm", "state": values(json!([0])), "action": null, "gripper": null},
            {"name": "arm", "state": values(json!([1])), "action": null, "gripper": null}
        ]}));
        assert!(twins.validate().is_err());
        assert!(roles(json!({"arms": []})).validate().is_ok());
    }
}
