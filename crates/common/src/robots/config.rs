//! A robot's saved configuration, validated whenever it is built or decoded.
//!
//! Each motor carries its own measured range, so a calibration cannot name a
//! different bus ID or direction than the motor it was measured on.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::visualization::RobotVisualization;
use super::{CameraMount, PortSettings};

/// One robot instance: exactly one arm, its motor bus, and its cameras.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RobotConfigFields")]
pub struct RobotConfig(RobotConfigFields);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RobotConfigFields {
    robot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    robot_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<PortSettings>,
    motors: BTreeMap<String, MotorConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    cameras: BTreeMap<String, CameraConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visualization: Option<RobotVisualization>,
}

impl TryFrom<RobotConfigFields> for RobotConfig {
    type Error = String;

    fn try_from(fields: RobotConfigFields) -> Result<Self, String> {
        fields.validate()?;
        Ok(Self(fields))
    }
}

impl RobotConfigFields {
    fn validate(&self) -> Result<(), String> {
        Self::validate_identifier("Robot ID", &self.robot_id)?;
        if let Some(kind) = &self.robot_type
            && (kind.trim().is_empty() || kind.len() > 64 || kind.chars().any(char::is_control))
        {
            return Err(
                "Robot type must contain 1–64 characters without control characters".into(),
            );
        }
        if let Some(port) = &self.port {
            port.validate()?;
        }
        if self.motors.is_empty() {
            return Err("A robot needs at least one motor".into());
        }
        let mut ids = BTreeMap::new();
        for (name, motor) in &self.motors {
            if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!(
                    "Motor name {name:?} must be nonempty without whitespace or control characters"
                ));
            }
            if let Some(first) = ids.insert(motor.id, name) {
                return Err(format!(
                    "Motor ID {} is assigned to both {first:?} and {name:?}",
                    motor.id
                ));
            }
            if let Some(range) = &motor.calibration {
                range
                    .validate()
                    .map_err(|error| format!("Motor {name:?}: {error}"))?;
            }
        }
        for (role, camera) in &self.cameras {
            Self::validate_identifier("Camera role", role)?;
            if camera.id.trim().is_empty() || camera.id.contains('\0') {
                return Err(format!(
                    "Camera {role:?} needs a device ID without NUL bytes"
                ));
            }
        }
        Ok(())
    }

    fn validate_identifier(kind: &str, value: &str) -> Result<(), String> {
        if (1..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            Ok(())
        } else {
            Err(format!(
                "{kind} {value:?} must contain 1–64 letters, digits, '-' or '_'"
            ))
        }
    }
}

impl RobotConfig {
    /// A new SO-101 arm with its stock motor layout and nothing measured yet.
    pub fn so101(id: &str, robot_type: &str) -> Result<Self, String> {
        Self::try_from(RobotConfigFields {
            robot_id: id.trim().to_owned(),
            robot_type: Some(robot_type.to_owned()),
            port: None,
            motors: MotorConfig::so101_arm(),
            cameras: BTreeMap::new(),
            visualization: None,
        })
    }

    pub fn id(&self) -> &str {
        &self.0.robot_id
    }
    pub fn robot_type(&self) -> Option<&str> {
        self.0.robot_type.as_deref()
    }
    pub const fn port(&self) -> Option<&PortSettings> {
        self.0.port.as_ref()
    }
    pub const fn motors(&self) -> &BTreeMap<String, MotorConfig> {
        &self.0.motors
    }
    /// Motors by bus ID, which for an SO-101 is also their order along the arm.
    pub fn motors_in_bus_order(&self) -> Vec<(&String, &MotorConfig)> {
        let mut motors = self.0.motors.iter().collect::<Vec<_>>();
        motors.sort_by_key(|(name, motor)| (motor.id, *name));
        motors
    }
    pub const fn cameras(&self) -> &BTreeMap<String, CameraConfig> {
        &self.0.cameras
    }
    pub const fn visualization(&self) -> Option<&RobotVisualization> {
        self.0.visualization.as_ref()
    }

    /// Every motor has a measured range.
    pub fn calibrated(&self) -> bool {
        self.0
            .motors
            .values()
            .all(|motor| motor.calibration.is_some())
    }

    pub fn with_port(mut self, port: PortSettings) -> Result<Self, String> {
        self.0.port = Some(port);
        Self::try_from(self.0)
    }

    /// Point `role` at `device`, keeping where that role's camera is mounted.
    ///
    /// A mount is written by hand into the configuration and nothing in the
    /// Studio edits it, so reassigning a role must leave it in place.
    pub fn with_camera(mut self, role: &str, device: &str) -> Result<Self, String> {
        let mount = self.0.cameras.remove(role).and_then(|camera| camera.mount);
        let camera = CameraConfig {
            id: device.to_owned(),
            mount,
        };
        self.0.cameras.insert(role.to_owned(), camera);
        Self::try_from(self.0)
    }

    pub fn without_camera(mut self, role: &str) -> Self {
        self.0.cameras.remove(role);
        self
    }

    /// Record one sweep's ranges, which must cover exactly the configured motors.
    pub fn with_calibration(
        mut self,
        ranges: BTreeMap<String, MotorRange>,
    ) -> Result<Self, String> {
        let configured = self.0.motors.keys().collect::<BTreeSet<_>>();
        let measured = ranges.keys().collect::<BTreeSet<_>>();
        if configured != measured {
            let list = |names: Vec<&&String>| {
                names
                    .into_iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(format!(
                "Calibration must measure exactly the configured motors; missing: [{}], unexpected: [{}]",
                list(configured.difference(&measured).collect()),
                list(measured.difference(&configured).collect()),
            ));
        }
        for (name, range) in ranges {
            if let Some(motor) = self.0.motors.get_mut(&name) {
                motor.calibration = Some(range);
            }
        }
        Self::try_from(self.0)
    }

    /// Whether saving `self` over `saved` changes what a connected motor bus uses.
    pub fn changes_control_configuration(&self, saved: &Self) -> bool {
        self.0.port != saved.0.port || self.0.motors != saved.0.motors
    }
}

/// One motor on the robot's bus.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotorConfig {
    id: u32,
    #[serde(default)]
    direction: MotorDirection,
    /// Selects a compatible bus driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// Raw travel measured by this motor's last calibration sweep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calibration: Option<MotorRange>,
}

impl MotorConfig {
    pub fn new(id: u32, direction: MotorDirection) -> Self {
        Self {
            id,
            direction,
            model: None,
            calibration: None,
        }
    }

    /// The stock SO-101 motor layout.
    pub fn so101_arm() -> BTreeMap<String, Self> {
        [
            ("shoulder_pan", 1),
            ("shoulder_lift", 2),
            ("elbow_flex", 3),
            ("wrist_flex", 4),
            ("wrist_roll", 5),
            ("gripper", 6),
        ]
        .into_iter()
        .map(|(name, id)| {
            (
                name.to_owned(),
                Self::new(id, MotorDirection::Normal).with_model("sts3215"),
            )
        })
        .collect()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub const fn with_calibration(mut self, range: MotorRange) -> Self {
        self.calibration = Some(range);
        self
    }

    pub const fn id(&self) -> u32 {
        self.id
    }
    pub const fn direction(&self) -> MotorDirection {
        self.direction
    }
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    pub const fn calibration(&self) -> Option<MotorRange> {
        self.calibration
    }
}

/// Whether increasing raw readings move the joint in its normal or reversed direction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorDirection {
    #[default]
    Normal,
    Reversed,
}

/// Raw, unhomed encoder travel of one motor, with its center strictly inside.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotorRange {
    range_min: i64,
    center: i64,
    range_max: i64,
}

impl MotorRange {
    pub fn new(range_min: i64, center: i64, range_max: i64) -> Result<Self, String> {
        let range = Self {
            range_min,
            center,
            range_max,
        };
        range.validate()?;
        Ok(range)
    }

    fn validate(&self) -> Result<(), String> {
        if self.range_min < self.center && self.center < self.range_max {
            Ok(())
        } else {
            Err(format!(
                "range must satisfy range_min < center < range_max, got {} < {} < {}",
                self.range_min, self.center, self.range_max
            ))
        }
    }

    pub const fn range_min(self) -> i64 {
        self.range_min
    }
    pub const fn center(self) -> i64 {
        self.center
    }
    pub const fn range_max(self) -> i64 {
        self.range_max
    }
}

/// A camera assigned to one of the robot's roles, such as `wrist` or `top`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraConfig {
    /// Stable device identifier reported by camera discovery.
    id: String,
    /// Where the camera is bolted, when the robot's file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mount: Option<CameraMount>,
}

impl CameraConfig {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn mount(&self) -> Option<&CameraMount> {
        self.mount.as_ref()
    }
}

/// Save a complete configuration over the revision its editor started from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotConfigSave {
    config: RobotConfig,
    /// `None` creates a new robot; otherwise the save fails if another write landed first.
    expected_revision: Option<u64>,
}

impl RobotConfigSave {
    pub fn new(config: RobotConfig, expected_revision: Option<u64>) -> Self {
        Self {
            config,
            expected_revision,
        }
    }
    pub const fn config(&self) -> &RobotConfig {
        &self.config
    }
    pub const fn expected_revision(&self) -> Option<u64> {
        self.expected_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(value: serde_json::Value) -> Result<RobotConfig, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn decoding_validates_identity_port_motors_calibration_and_cameras() {
        let motors = json!({"joint": {"id": 1}});
        for invalid in [
            json!({"robot_id": "../arm", "motors": motors}),
            json!({"robot_id": "arm", "motors": {}}),
            json!({"robot_id": "arm", "motors": {"a": {"id": 1}, "b": {"id": 1}}}),
            json!({"robot_id": "arm", "motors": {"joint": {"id": 1, "direction": "sideways"}}}),
            json!({"robot_id": "arm", "motors": {"joint": {"id": 1, "calibration": {"range_min": 5, "center": 5, "range_max": 9}}}}),
            json!({"robot_id": "arm", "motors": motors, "port": {"path": "/dev/arm", "baud_rate": 0}}),
            json!({"robot_id": "arm", "motors": motors, "cameras": {"bad role": {"id": "0"}}}),
            json!({"robot_id": "arm", "motors": motors, "extension": 42}),
        ] {
            assert!(decode(invalid.clone()).is_err(), "accepted {invalid}");
        }
        let config = decode(json!({"robot_id": "arm", "motors": motors})).unwrap();
        assert!(!config.calibrated());
        assert_eq!(config.motors()["joint"].direction(), MotorDirection::Normal);
    }

    #[test]
    fn calibration_must_cover_exactly_the_configured_motors() {
        let config = RobotConfig::so101("arm", "so101_follower").unwrap();
        let range = MotorRange::new(100, 500, 900).unwrap();
        let mut ranges = config
            .motors()
            .keys()
            .map(|name| (name.clone(), range))
            .collect::<BTreeMap<_, _>>();
        ranges.remove("gripper");
        assert!(config.clone().with_calibration(ranges.clone()).is_err());
        ranges.insert("gripper".to_owned(), range);
        let calibrated = config.clone().with_calibration(ranges).unwrap();
        assert!(calibrated.calibrated());
        assert!(calibrated.changes_control_configuration(&config));
        let encoded = serde_json::to_value(&calibrated).unwrap();
        assert_eq!(encoded["motors"]["gripper"]["calibration"]["center"], 500);
        assert_eq!(decode(encoded).unwrap(), calibrated);
    }

    #[test]
    fn reassigning_a_camera_role_keeps_its_mount() {
        let mounted = decode(json!({
            "robot_id": "arm",
            "motors": {"joint": {"id": 1}},
            "cameras": {"wrist": {"id": "/dev/video0", "mount": {"frame": "gripper_link"}}}
        }))
        .unwrap();
        let moved = mounted.with_camera("wrist", "/dev/video9").unwrap();
        assert_eq!(moved.cameras()["wrist"].id(), "/dev/video9");
        assert_eq!(
            moved.cameras()["wrist"]
                .mount()
                .and_then(CameraMount::frame),
            Some("gripper_link")
        );
        assert!(moved.clone().with_camera("bad role", "0").is_err());
        assert!(moved.without_camera("wrist").cameras().is_empty());
    }

    #[test]
    fn camera_edits_leave_the_control_configuration_unchanged() {
        let config = RobotConfig::so101("arm", "so101_follower").unwrap();
        let edited = config.clone().with_camera("wrist", "/dev/video0").unwrap();
        assert!(!edited.changes_control_configuration(&config));
        let moved = config
            .clone()
            .with_port(PortSettings::new("/dev/arm", 1_000_000))
            .unwrap();
        assert!(moved.changes_control_configuration(&config));
    }
}
