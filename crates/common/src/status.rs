//! Studio bootstrap status and top-level snapshot.

use crate::command_failure::CommandFailureCode;
use serde::{Deserialize, Serialize};

use crate::robots::{CameraStatus, ModeStatus, RobotSummary};

/// Everything the UI needs to draw the setup screens.
///
/// This changes only when a robot, camera, or mode changes, so it is sent on
/// connect and then whenever it actually differs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StudioSnapshot {
    /// Whether backend-owned services are still starting, usable, or running
    /// with a non-fatal bootstrap problem.
    #[serde(default)]
    readiness: BackendReadiness,
    /// Human-readable name of the active persistent store.
    storage: String,
    /// Every robot configuration found in SpiralDB, by ID.
    robots: Vec<RobotSummary>,
    /// Cameras the server has discovered.
    cameras: Vec<CameraStatus>,
    /// The currently selected operating mode.
    mode: ModeStatus,
    /// Problems worth showing the operator: invalid robot configurations, cameras
    /// that failed to probe, and similar non-fatal trouble.
    issues: Vec<String>,
}

/// Lifecycle of the backend services needed by the browser application.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendReadinessState {
    #[default]
    Bootstrapping,
    Ready,
    Degraded,
}

/// Browser-visible readiness of the backend and its Spiral catalog boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendReadiness {
    state: BackendReadinessState,
    /// True once the configured SpiralDB session has been warmed enough for
    /// independent dataset and episode reads.
    #[serde(default)]
    storage_available: bool,
    /// True once preferences and run managers are hydrated and the complete
    /// catalog-dependent API may safely be admitted.
    catalog_available: bool,
    /// Short operator-facing description of the current phase or problem.
    detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_code: Option<CommandFailureCode>,
}

impl Default for BackendReadiness {
    fn default() -> Self {
        Self {
            state: BackendReadinessState::Bootstrapping,
            storage_available: false,
            catalog_available: false,
            detail: "Starting Studio services".to_owned(),
            failure_code: None,
        }
    }
}

impl BackendReadiness {
    pub fn new(
        state: BackendReadinessState,
        catalog_available: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            state,
            storage_available: catalog_available,
            catalog_available,
            detail: detail.into(),
            failure_code: None,
        }
    }
    /// Mark the SpiralDB read boundary ready while the remaining Studio
    /// services continue hydrating.
    pub fn with_storage_available(mut self) -> Self {
        self.storage_available = true;
        self
    }
    pub const fn state(&self) -> BackendReadinessState {
        self.state
    }
    pub const fn storage_available(&self) -> bool {
        self.storage_available
    }
    pub const fn catalog_available(&self) -> bool {
        self.catalog_available
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
    pub fn with_failure_code(mut self, code: CommandFailureCode) -> Self {
        self.failure_code = Some(code);
        self
    }
    pub const fn failure_code(&self) -> Option<CommandFailureCode> {
        self.failure_code
    }
}

impl StudioSnapshot {
    pub fn new(readiness: BackendReadiness, storage: impl Into<String>, mode: ModeStatus) -> Self {
        Self {
            readiness,
            storage: storage.into(),
            robots: Vec::new(),
            cameras: Vec::new(),
            mode,
            issues: Vec::new(),
        }
    }
    pub fn with_robots(mut self, robots: Vec<RobotSummary>) -> Self {
        self.robots = robots;
        self
    }
    pub fn with_cameras(mut self, cameras: Vec<CameraStatus>) -> Self {
        self.cameras = cameras;
        self
    }
    pub fn with_issues(mut self, issues: Vec<String>) -> Self {
        self.issues = issues;
        self
    }
    pub const fn readiness(&self) -> &BackendReadiness {
        &self.readiness
    }
    pub fn robots(&self) -> &[RobotSummary] {
        &self.robots
    }
    pub fn cameras(&self) -> &[CameraStatus] {
        &self.cameras
    }
    pub const fn mode(&self) -> &ModeStatus {
        &self.mode
    }
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
    /// Pick the best connected pair for a recording-led teleoperation session.
    ///
    /// Robot type is a preference rather than a requirement: a configured leader
    /// and follower win, while two otherwise ready compatible arms still work.
    /// Compatibility is checked here so the Record button and the server make the
    /// same decision instead of offering a pair the control thread must reject.
    pub fn default_teleoperation_pair(&self) -> Option<(String, String)> {
        let ready = self
            .robots
            .iter()
            .filter(|robot| {
                robot.connection().connected() && robot.calibrated() && robot.problem().is_none()
            })
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
                            && leader.motors().iter().any(|leader_motor| {
                                follower
                                    .motors()
                                    .iter()
                                    .any(|motor| motor.name() == leader_motor.name())
                            })
                    })
                    .map(move |(follower_index, follower)| {
                        let leader_has_role = leader
                            .robot_type()
                            .is_some_and(|kind| kind.to_ascii_lowercase().contains("leader"));
                        let follower_has_role = follower
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::robots::{ConnectionStatus, MotorSummary, RobotAvailability};

    struct StudioSnapshotTest;

    impl StudioSnapshotTest {
        fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
            RobotSummary::new(
                id,
                robot_type.map(str::to_owned),
                RobotAvailability::new(true, None, ConnectionStatus::new(true, None)),
            )
            .with_motors(vec![MotorSummary::new("shoulder", 1)])
        }
    }

    #[test]
    fn storage_reads_can_be_ready_while_services_are_still_hydrating() {
        let readiness = BackendReadiness::new(
            BackendReadinessState::Bootstrapping,
            false,
            "Loading Studio preferences and run history",
        )
        .with_storage_available();

        assert!(readiness.storage_available());
        assert!(!readiness.catalog_available());
        assert_eq!(readiness.state(), BackendReadinessState::Bootstrapping);
    }

    #[test]
    fn teleoperation_pair_prefers_configured_roles_over_list_order() {
        let studio = StudioSnapshot {
            robots: vec![
                StudioSnapshotTest::robot("follower", Some("so101_follower")),
                StudioSnapshotTest::robot("spare", None),
                StudioSnapshotTest::robot("leader", Some("SO101_LEADER")),
            ],
            ..StudioSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some(("leader".to_owned(), "follower".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_falls_back_to_the_first_compatible_ready_arms() {
        let studio = StudioSnapshot {
            robots: vec![
                StudioSnapshotTest::robot("left", None),
                StudioSnapshotTest::robot("right", None),
            ],
            ..StudioSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some(("left".to_owned(), "right".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_ignores_unready_and_incompatible_arms() {
        let disconnected = RobotSummary::new(
            "disconnected-leader",
            Some("leader".to_owned()),
            RobotAvailability::new(true, None, ConnectionStatus::new(false, None)),
        )
        .with_motors(vec![MotorSummary::new("shoulder", 1)]);
        let incompatible = StudioSnapshotTest::robot("incompatible-leader", Some("leader"))
            .with_motors(vec![MotorSummary::new("elbow", 1)]);
        let follower = StudioSnapshotTest::robot("follower", Some("follower"));
        let fallback = StudioSnapshotTest::robot("fallback", None);
        let studio = StudioSnapshot {
            robots: vec![disconnected, incompatible, follower, fallback.clone()],
            ..StudioSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some((fallback.id().to_owned(), "follower".to_owned()))
        );
    }
}
