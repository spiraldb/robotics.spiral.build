//! Robot configuration and observed hardware for hardware feature views.
use spiral_robotics_common::robots::{CameraStatus, ModeStatus, RobotSummary};

/// Everything the UI needs to draw the setup screens.
///
/// This changes only when a robot, camera, or mode changes, so it is sent on
/// connect and then whenever it actually differs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RobotSetupSnapshot {
    /// Every robot configuration saved in Studio, by ID.
    robots: Vec<RobotSummary>,
    /// Cameras the server has discovered.
    cameras: Vec<CameraStatus>,
    /// The currently selected operating mode.
    mode: ModeStatus,
    /// Problems worth showing the operator: invalid robot configurations, cameras
    /// that failed to probe, and similar non-fatal trouble.
    issues: Vec<String>,
}

impl RobotSetupSnapshot {
    pub(super) fn new(mode: ModeStatus) -> Self {
        Self {
            robots: Vec::new(),
            cameras: Vec::new(),
            mode,
            issues: Vec::new(),
        }
    }
    pub(super) fn with_robots(mut self, robots: Vec<RobotSummary>) -> Self {
        self.robots = robots;
        self
    }
    pub(super) fn with_cameras(mut self, cameras: Vec<CameraStatus>) -> Self {
        self.cameras = cameras;
        self
    }
    pub(super) fn with_issues(mut self, issues: Vec<String>) -> Self {
        self.issues = issues;
        self
    }
    pub(crate) fn robots(&self) -> &[RobotSummary] {
        &self.robots
    }
    pub(crate) fn cameras(&self) -> &[CameraStatus] {
        &self.cameras
    }
    pub(crate) const fn mode(&self) -> &ModeStatus {
        &self.mode
    }
    pub(crate) fn issues(&self) -> &[String] {
        &self.issues
    }
    /// Pick the best connected pair for a recording-led teleoperation session.
    ///
    /// Robot type is a preference rather than a requirement: a configured leader
    /// and follower win, while two otherwise ready compatible arms still work.
    /// The selected identities are submitted explicitly; hardware revalidates admission.
    pub(crate) fn default_teleoperation_pair(&self) -> Option<(String, String)> {
        let ready = self
            .robots
            .iter()
            .filter(|robot| robot.connection().connected() && robot.config().calibrated())
            .collect::<Vec<_>>();
        ready
            .iter()
            .enumerate()
            .flat_map(|(leader_index, leader)| {
                ready
                    .iter()
                    .enumerate()
                    .filter(move |(follower_index, follower)| {
                        leader_index != *follower_index
                            && leader
                                .config()
                                .motors()
                                .keys()
                                .any(|name| follower.config().motors().contains_key(name))
                    })
                    .map(move |(follower_index, follower)| {
                        let leader_has_role = leader
                            .config()
                            .robot_type()
                            .is_some_and(|kind| kind.to_ascii_lowercase().contains("leader"));
                        let follower_has_role = follower
                            .config()
                            .robot_type()
                            .is_some_and(|kind| kind.to_ascii_lowercase().contains("follower"));
                        (
                            (
                                u8::from(!leader_has_role) + u8::from(!follower_has_role),
                                leader_index,
                                follower_index,
                            ),
                            (leader.id().to_owned(), follower.id().to_owned()),
                        )
                    })
            })
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, pair)| pair)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::robots::ConnectionStatus;
    use spiral_robotics_common::robots::config::RobotConfig;

    struct RobotSetupSnapshotTest;

    impl RobotSetupSnapshotTest {
        /// A one-motor arm, calibrated, of an optional type.
        fn config(id: &str, robot_type: Option<&str>, motor: &str) -> RobotConfig {
            serde_json::from_value(serde_json::json!({
                "robot_id": id,
                "robot_type": robot_type,
                "motors": {motor: {"id": 1, "calibration": {"range_min": 0, "center": 1, "range_max": 2}}},
            }))
            .unwrap()
        }

        fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
            RobotSummary::new(
                Self::config(id, robot_type, "shoulder"),
                1,
                ConnectionStatus::new(true, None),
            )
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn teleoperation_pair_prefers_configured_roles_over_list_order() {
        let studio = RobotSetupSnapshot {
            robots: vec![
                RobotSetupSnapshotTest::robot("follower", Some("so101_follower")),
                RobotSetupSnapshotTest::robot("spare", None),
                RobotSetupSnapshotTest::robot("leader", Some("SO101_LEADER")),
            ],
            ..RobotSetupSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some(("leader".to_owned(), "follower".to_owned()))
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn teleoperation_pair_falls_back_to_the_first_compatible_ready_arms() {
        let studio = RobotSetupSnapshot {
            robots: vec![
                RobotSetupSnapshotTest::robot("left", None),
                RobotSetupSnapshotTest::robot("right", None),
            ],
            ..RobotSetupSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some(("left".to_owned(), "right".to_owned()))
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn teleoperation_pair_ignores_unready_and_incompatible_arms() {
        let disconnected = RobotSummary::new(
            RobotSetupSnapshotTest::config("disconnected-leader", Some("leader"), "shoulder"),
            1,
            ConnectionStatus::new(false, None),
        );
        let incompatible = RobotSummary::new(
            RobotSetupSnapshotTest::config("incompatible-leader", Some("leader"), "elbow"),
            1,
            ConnectionStatus::new(true, None),
        );
        let follower = RobotSetupSnapshotTest::robot("follower", Some("follower"));
        let fallback = RobotSetupSnapshotTest::robot("fallback", None);
        let studio = RobotSetupSnapshot {
            robots: vec![disconnected, incompatible, follower, fallback.clone()],
            ..RobotSetupSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some((fallback.id().to_owned(), "follower".to_owned()))
        );
    }
}
