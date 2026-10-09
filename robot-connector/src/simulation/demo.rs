//! The demo bench Studio installs: a simulated SO-101 leader a simulated hand
//! moves, and a follower filmed by a wrist camera and a front camera.
//!
//! Both are ordinary saved robots, so everything Studio does with real arms —
//! teleoperation, takes with video, rollout — works on them unchanged. The
//! model files are TheRobotStudio's SO-101 description; see `so101/README.md`.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use base64::Engine as _;
use serde_json::json;
use spiral_robotics_common::robots::config::{MotorRange, RobotConfig};
use spiral_robotics_common::robots::visualization::{RobotModelFiles, RobotModelId};
use spiral_robotics_viewport::RobotModel;

use crate::robots::workspace::{RobotWorkspace, WorkspaceError};

/// The bundled SO-101 meshes, by the filename the URDFs give them.
const MESHES: [(&str, &[u8]); 16] = [
    (
        "assets/Handle_SO101.stl",
        include_bytes!("so101/assets/Handle_SO101.stl"),
    ),
    (
        "assets/Trigger_SO101.stl",
        include_bytes!("so101/assets/Trigger_SO101.stl"),
    ),
    (
        "assets/Wrist_Roll_SO101.stl",
        include_bytes!("so101/assets/Wrist_Roll_SO101.stl"),
    ),
    (
        "assets/base_motor_holder_so101_v1.stl",
        include_bytes!("so101/assets/base_motor_holder_so101_v1.stl"),
    ),
    (
        "assets/base_so101_v2.stl",
        include_bytes!("so101/assets/base_so101_v2.stl"),
    ),
    (
        "assets/motor_holder_so101_base_v1.stl",
        include_bytes!("so101/assets/motor_holder_so101_base_v1.stl"),
    ),
    (
        "assets/motor_holder_so101_wrist_v1.stl",
        include_bytes!("so101/assets/motor_holder_so101_wrist_v1.stl"),
    ),
    (
        "assets/moving_jaw_so101_v1.stl",
        include_bytes!("so101/assets/moving_jaw_so101_v1.stl"),
    ),
    (
        "assets/rotation_pitch_so101_v1.stl",
        include_bytes!("so101/assets/rotation_pitch_so101_v1.stl"),
    ),
    (
        "assets/sts3215_03a_no_horn_v1.stl",
        include_bytes!("so101/assets/sts3215_03a_no_horn_v1.stl"),
    ),
    (
        "assets/sts3215_03a_v1.stl",
        include_bytes!("so101/assets/sts3215_03a_v1.stl"),
    ),
    (
        "assets/under_arm_so101_v1.stl",
        include_bytes!("so101/assets/under_arm_so101_v1.stl"),
    ),
    (
        "assets/upper_arm_so101_v1.stl",
        include_bytes!("so101/assets/upper_arm_so101_v1.stl"),
    ),
    (
        "assets/waveshare_mounting_plate_so101_v2.stl",
        include_bytes!("so101/assets/waveshare_mounting_plate_so101_v2.stl"),
    ),
    (
        "assets/wrist_roll_follower_so101_v1.stl",
        include_bytes!("so101/assets/wrist_roll_follower_so101_v1.stl"),
    ),
    (
        "assets/wrist_roll_pitch_so101_v2.stl",
        include_bytes!("so101/assets/wrist_roll_pitch_so101_v2.stl"),
    ),
];

/// Encoder counts in one turn of an STS3215, which the simulated motors share.
const COUNTS_PER_TURN: f64 = 4096.0;

/// Where every simulated motor's encoder reads when its joint is centred.
const CENTER_COUNTS: i64 = 2048;

pub(crate) struct DemoBench;

impl DemoBench {
    /// The robots the bench saves: a hand-guided leader and a filmed follower.
    pub(crate) const ROBOTS: [&'static str; 2] = [DemoArm::LEADER.id, DemoArm::FOLLOWER.id];

    /// Save the demo arms and their models over whatever this data directory
    /// held for them, so a demo always starts from the bench defined here.
    pub(crate) async fn install(workspace: &RobotWorkspace) -> Result<(), WorkspaceError> {
        for arm in [DemoArm::LEADER, DemoArm::FOLLOWER] {
            arm.install(workspace).await?;
        }
        Ok(())
    }
}

/// One arm of the demo bench.
struct DemoArm {
    id: &'static str,
    robot_type: &'static str,
    urdf: &'static str,
    hand_guided: bool,
    cameras: &'static [DemoCamera],
}

/// A camera on the demo bench and where it is mounted, in the optical
/// convention: looking along +Z with +Y down the picture.
struct DemoCamera {
    role: &'static str,
    /// The link it rides, or `None` for a camera standing on the bench.
    frame: Option<&'static str>,
    xyz: [f64; 3],
    rpy: [f64; 3],
}

impl DemoArm {
    const LEADER: Self = Self {
        id: "sim_leader",
        robot_type: "so101_leader",
        urdf: include_str!("so101/so101_leader.urdf"),
        hand_guided: true,
        cameras: &[],
    };

    const FOLLOWER: Self = Self {
        id: "sim_follower",
        robot_type: "so101_follower",
        urdf: include_str!("so101/so101_follower.urdf"),
        hand_guided: false,
        cameras: &[
            // Beside the fixed jaw, looking down the gripper past its tips.
            DemoCamera {
                role: "wrist",
                frame: Some("gripper_link"),
                xyz: [0.0, 0.045, -0.025],
                rpy: [PI - 0.35, 0.0, 0.0],
            },
            // Across the bench on the side away from the leader, looking down
            // at the follower's reach from (0.48, -0.50, 0.46) to (0.14, 0, 0.10).
            DemoCamera {
                role: "front",
                frame: None,
                xyz: [0.48, -0.50, 0.46],
                rpy: [-2.1948, -0.5043, -0.3348],
            },
        ],
    };

    async fn install(&self, workspace: &RobotWorkspace) -> Result<(), WorkspaceError> {
        let model = workspace.models().save(&self.model()?)?;
        let config = self.config(&model)?;
        let saved = workspace
            .get(self.id)
            .await
            .ok()
            .map(|record| record.revision());
        workspace.save(config, saved).await?;
        Ok(())
    }

    /// The URDF and every bundled mesh it names.
    fn model(&self) -> Result<RobotModelFiles, WorkspaceError> {
        let meshes = RobotModel::mesh_files(self.urdf)
            .map_err(WorkspaceError::Invalid)?
            .into_iter()
            .map(|name| {
                let (_, bytes) = MESHES
                    .iter()
                    .find(|(bundled, _)| *bundled == name)
                    .ok_or_else(|| WorkspaceError::Invalid(format!("{name} is not bundled")))?;
                Ok((
                    name,
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, WorkspaceError>>()?;
        RobotModelFiles::new(self.urdf.to_owned(), meshes).map_err(WorkspaceError::Invalid)
    }

    /// An SO-101 calibrated to its model's joint limits, its encoders and
    /// the model agreeing on every angle.
    fn config(&self, model: &RobotModelId) -> Result<RobotConfig, WorkspaceError> {
        let joints = JointTravel::of(self.urdf)?;
        let calibrated = RobotConfig::so101(self.id, self.robot_type)
            .and_then(|config| {
                config.with_calibration(
                    joints
                        .iter()
                        .map(|(name, travel)| Ok((name.clone(), travel.range()?)))
                        .collect::<Result<_, String>>()?,
                )
            })
            .map_err(WorkspaceError::Invalid)?;
        let mut document = serde_json::to_value(calibrated)
            .map_err(|error| WorkspaceError::Invalid(error.to_string()))?;
        document["simulated"] = json!({ "hand_guided": self.hand_guided });
        document["cameras"] = self
            .cameras
            .iter()
            .map(|camera| (camera.role.to_owned(), camera.config(self.id)))
            .collect();
        document["visualization"] = json!({
            "model": model,
            "joints": joints
                .iter()
                .map(|(name, travel)| (name.clone(), travel.coordinate()))
                .collect::<serde_json::Map<_, _>>(),
            "tool_link": "gripper_frame_link",
        });
        serde_json::from_value(document).map_err(|error| WorkspaceError::Invalid(error.to_string()))
    }
}

impl DemoCamera {
    fn config(&self, robot: &str) -> serde_json::Value {
        json!({
            "id": format!("simulated/{robot}/{}", self.role),
            "mount": { "frame": self.frame, "xyz": self.xyz, "rpy": self.rpy },
        })
    }
}

/// A joint's travel in its model, in radians.
struct JointTravel {
    lower: f64,
    upper: f64,
}

impl JointTravel {
    fn of(urdf: &str) -> Result<BTreeMap<String, Self>, WorkspaceError> {
        Ok(RobotModel::joint_limits(urdf)
            .map_err(WorkspaceError::Invalid)?
            .into_iter()
            .map(|(name, (lower, upper))| {
                let travel = Self {
                    lower: f64::from(lower),
                    upper: f64::from(upper),
                };
                (name, travel)
            })
            .collect())
    }

    /// The angle the encoder centre stands for: the model's zero, unless the
    /// joint cannot reach it, as a leader's trigger cannot.
    fn center(&self) -> f64 {
        if self.lower < 0.0 && 0.0 < self.upper {
            0.0
        } else {
            (self.lower + self.upper) / 2.0
        }
    }

    fn range(&self) -> Result<MotorRange, String> {
        let counts = |angle: f64| {
            CENTER_COUNTS + ((angle - self.center()) * COUNTS_PER_TURN / (2.0 * PI)).round() as i64
        };
        MotorRange::new(counts(self.lower), CENTER_COUNTS, counts(self.upper))
    }

    /// Telemetry reads degrees from the encoder centre; the model wants radians.
    fn coordinate(&self) -> serde_json::Value {
        json!({
            "input": "degrees",
            "scale": PI / 180.0,
            "offset": self.center(),
            "rest": self.center(),
        })
    }
}
