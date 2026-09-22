//! A dataset's resolved visualization, independent of connected hardware.
use super::DatasetSelection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A dataset's resolved robot, or `None` when it supplies no usable model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetRobotSave {
    dataset: DatasetSelection,
    robot: Option<DatasetRobot>,
}
impl DatasetRobotSave {
    pub fn new(dataset: DatasetSelection, robot: Option<DatasetRobot>) -> Self {
        Self { dataset, robot }
    }
    pub fn into_parts(self) -> (DatasetSelection, Option<DatasetRobot>) {
        (self.dataset, self.robot)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetRobot {
    urdf: String,
    bindings: Vec<JointBinding>,
    /// Base64 mesh bytes, keyed by the exact URDF filename.
    #[serde(default)]
    meshes: BTreeMap<String, String>,
}

impl DatasetRobot {
    pub fn urdf(&self) -> &str {
        &self.urdf
    }
    pub fn bindings(&self) -> &[JointBinding] {
        &self.bindings
    }
    pub fn meshes(&self) -> &BTreeMap<String, String> {
        &self.meshes
    }
    pub fn set_meshes(&mut self, meshes: BTreeMap<String, String>) {
        self.meshes = meshes;
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.urdf.is_empty() || self.urdf.len() > 2 * 1024 * 1024 || self.bindings.len() > 128 {
            return Err("Robot description exceeds its supported size".into());
        }
        let mut joints = std::collections::BTreeSet::new();
        for binding in &self.bindings {
            if binding.joint.is_empty()
                || binding.topic.is_empty()
                || binding.schema.is_empty()
                || binding.field.is_empty()
                || binding.field.iter().any(String::is_empty)
                || matches!(&binding.selector, Some(JointSelector::Named { names, name }) if names.is_empty() || names.iter().any(String::is_empty) || name.is_empty())
                || !binding.scale.is_finite()
                || !binding.offset.is_finite()
                || !joints.insert(&binding.joint)
            {
                return Err("Invalid or duplicate robot joint binding".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JointBinding {
    joint: String,
    topic: String,
    schema: String,
    /// Structured path below the recorded message's value.
    field: Vec<String>,
    selector: Option<JointSelector>,
    /// Convert recorded units to URDF radians/metres: value * scale + offset.
    scale: f64,
    offset: f64,
}
impl JointBinding {
    pub fn read_position(&self, message: &serde_json::Value) -> Option<f32> {
        let value = Self::field_value(message, &self.field)?;
        let selected = match &self.selector {
            None => &value,
            Some(JointSelector::Index { index }) => value.as_array()?.get(*index)?,
            Some(JointSelector::Named { names, name }) => {
                let names = Self::field_value(message, names)?;
                let mut matching = names
                    .as_array()?
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| value.as_str() == Some(name));
                let index = matching.next()?.0;
                if matching.next().is_some() {
                    return None;
                }
                value.as_array()?.get(index)?
            }
        };
        self.position(selected.as_f64()?)
    }

    fn field_value(value: &serde_json::Value, path: &[String]) -> Option<serde_json::Value> {
        let Some((name, remaining)) = path.split_first() else {
            return Some(value.clone());
        };
        if let Some(items) = value.as_array() {
            return Some(serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| Self::field_value(item, path).unwrap_or(serde_json::Value::Null))
                    .collect(),
            ));
        }
        Self::field_value(value.get(name)?, remaining)
    }
    pub fn joint(&self) -> &str {
        &self.joint
    }
    pub fn topic(&self) -> &str {
        &self.topic
    }
    pub fn schema(&self) -> &str {
        &self.schema
    }
    fn position(&self, value: f64) -> Option<f32> {
        let position = (value * self.scale + self.offset) as f32;
        position.is_finite().then_some(position)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JointSelector {
    Index {
        index: usize,
    },
    /// Resolve the index in each message; named joint arrays may reorder.
    Named {
        names: Vec<String>,
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture;
    impl Fixture {
        fn binding(selector: serde_json::Value) -> JointBinding {
            serde_json::from_value(json!({
                "joint":"elbow", "topic":"/joints", "schema":"JointState",
                "field":["position"], "selector":selector, "scale":1.0, "offset":0.0
            }))
            .unwrap()
        }
    }

    #[test]
    fn named_arrays_follow_each_messages_order_and_reject_ambiguous_names() {
        let binding = Fixture::binding(json!({"kind":"named","names":["name"],"name":"elbow"}));
        assert_eq!(
            binding.read_position(&json!({"name":["wrist","elbow"],"position":[0.1,0.8]})),
            Some(0.8)
        );
        assert_eq!(
            binding.read_position(&json!({"name":["elbow","wrist"],"position":[0.3,0.2]})),
            Some(0.3)
        );
        assert_eq!(
            binding.read_position(&json!({"name":["elbow","elbow"],"position":[0.3,0.2]})),
            None
        );
        assert_eq!(
            binding.read_position(&json!({"name":["wrist"],"position":[0.3]})),
            None
        );
    }

    #[test]
    fn native_list_wrappers_preserve_missing_elements_and_convert_units() {
        let mut binding = Fixture::binding(json!({"kind":"index","index":1}));
        binding.field = vec!["joints".into(), "degrees".into()];
        binding.scale = std::f64::consts::PI / 180.0;
        binding.offset = -0.1;
        let message = json!({"joints":[{"degrees":null},{"degrees":90.0}]});
        assert!(
            (binding.read_position(&message).unwrap() - (std::f32::consts::FRAC_PI_2 - 0.1)).abs()
                < 1e-6
        );
        binding.selector = Some(JointSelector::Index { index: 0 });
        assert_eq!(binding.read_position(&message), None);
        assert_eq!(binding.read_position(&json!({})), None);
    }
}
