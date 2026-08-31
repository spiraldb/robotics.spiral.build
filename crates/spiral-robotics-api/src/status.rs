//! Studio bootstrap status, top-level snapshot, and request failures.

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
}

impl Default for BackendReadiness {
    fn default() -> Self {
        Self {
            state: BackendReadinessState::Bootstrapping,
            storage_available: false,
            catalog_available: false,
            detail: "Starting Studio services".to_owned(),
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

/// A failed request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    error: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }

    pub fn into_error(self) -> String {
        self.error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::*;
    use crate::robots::*;
    use crate::rollout::*;
    use crate::settings::*;
    use crate::training::*;

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
    fn act_defaults_match_current_lerobot_architecture() {
        let config = ActTrainingConfig::default();
        assert_eq!(config.hidden_dim(), 512);
        assert_eq!(config.feedforward_dim(), 3_200);
        assert_eq!(config.vae_encoder_layers(), 4);
        assert_eq!(config.decoder_layers(), 1);
        assert!(config.camera_streams().is_empty());
        let request = StartTrainingRequest::default();
        assert_eq!(request.batch_size(), 8);
        assert_eq!(request.steps(), 100_000);
        assert_eq!(request.checkpoint_every(), 20_000);
        assert_eq!(request.seed(), 1_000);
        assert_eq!(request.validation_split(), 0.1);
    }

    #[test]
    fn historical_training_requests_keep_no_validation_semantics() {
        let mut serialized_request = serde_json::to_value(StartTrainingRequest::default()).unwrap();
        serialized_request
            .as_object_mut()
            .unwrap()
            .remove("validation_split");
        let request: StartTrainingRequest = serde_json::from_value(serialized_request).unwrap();
        assert_eq!(request.validation_split(), 0.0);
    }

    #[test]
    fn older_rollout_requests_receive_the_safe_default_duration() {
        let request: StartRolloutRequest = serde_json::from_value(serde_json::json!({
            "run_id": "run-1",
            "checkpoint_step": 20_000,
            "robot": "follower"
        }))
        .expect("duration defaults for an older client");

        assert_eq!(request.duration_seconds(), DEFAULT_ROLLOUT_DURATION_SECONDS);
    }

    #[test]
    fn motor_summary_wire_excludes_hardware_configuration_details() {
        let summary = MotorSummary::new("joint", 7).with_model(Some("sts3215".to_owned()));

        let json = serde_json::to_string(&summary).unwrap();
        assert_eq!(
            json,
            r#"{"name":"joint","id":7,"model":"sts3215","limits":{"max_temperature_celsius":null,"min_voltage_volts":null,"max_voltage_volts":null,"max_load":null}}"#
        );
        assert_eq!(
            serde_json::from_str::<MotorSummary>(&json).unwrap(),
            summary
        );
    }

    fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
        RobotSummary::new(
            id,
            robot_type.map(str::to_owned),
            RobotAvailability::new(true, None, ConnectionStatus::new(true, None)),
        )
        .with_motors(vec![MotorSummary::new("shoulder", 1)])
    }

    #[test]
    fn teleoperation_pair_prefers_configured_roles_over_list_order() {
        let studio = StudioSnapshot {
            robots: vec![
                robot("follower", Some("so101_follower")),
                robot("spare", None),
                robot("leader", Some("SO101_LEADER")),
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
            robots: vec![robot("left", None), robot("right", None)],
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
        let incompatible = robot("incompatible-leader", Some("leader"))
            .with_motors(vec![MotorSummary::new("elbow", 1)]);
        let follower = robot("follower", Some("follower"));
        let fallback = robot("fallback", None);
        let studio = StudioSnapshot {
            robots: vec![disconnected, incompatible, follower, fallback.clone()],
            ..StudioSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some((fallback.id().to_owned(), "follower".to_owned()))
        );
    }

    #[test]
    fn mode_selection_round_trips_through_its_tag() {
        let cases = [
            (ModeSelection::Idle, r#"{"mode":"idle"}"#),
            (
                ModeSelection::Manual {
                    robot: "arm".to_owned(),
                },
                r#"{"mode":"manual","robot":"arm"}"#,
            ),
            (
                ModeSelection::Teleoperate {
                    leader: "left".to_owned(),
                    follower: "right".to_owned(),
                },
                r#"{"mode":"teleoperate","leader":"left","follower":"right"}"#,
            ),
            (
                ModeSelection::Replay {
                    robot: "right".to_owned(),
                    episode: "episode_000007".to_owned(),
                },
                r#"{"mode":"replay","robot":"right","episode":"episode_000007"}"#,
            ),
            (
                ModeSelection::Policy {
                    robot: "right".to_owned(),
                    session: "rollout-1".to_owned(),
                },
                r#"{"mode":"policy","robot":"right","session":"rollout-1"}"#,
            ),
        ];

        for (mode, json) in cases {
            assert_eq!(serde_json::to_string(&mode).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<ModeSelection>(json).unwrap(),
                mode,
                "{json} should round-trip"
            );
        }
    }

    #[test]
    fn optional_request_fields_may_be_omitted() {
        let created: CreateRobotRequest = serde_json::from_str(r#"{"id":"arm"}"#).unwrap();
        assert_eq!(created.id(), "arm");
        assert_eq!(created.robot_type(), None);

        let update: UpdateRobotRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(update, UpdateRobotRequest::default());

        let settings: UpdateDataSettingsRequest = serde_json::from_str(
            r#"{
                "video": { "enabled": true }
            }"#,
        )
        .unwrap();
        assert_eq!(settings.spiral_endpoint(), DEFAULT_SPIRAL_ENDPOINT);
        assert_eq!(
            settings.spiral_spfs_endpoint(),
            DEFAULT_SPIRAL_SPFS_ENDPOINT
        );
        assert_eq!(settings.training(), &TrainingRuntimeSettings::default());
    }

    #[test]
    fn older_replay_progress_defaults_to_timeline_playback() {
        let replay: ReplayProgress = serde_json::from_str(
            r#"{
                "episode":"episode_000001",
                "robot":"follower",
                "frame":12,
                "frames":60,
                "elapsed_seconds":0.2,
                "duration_seconds":1.0
            }"#,
        )
        .unwrap();

        assert_eq!(replay.purpose(), ReplayPurpose::Timeline);
        assert_eq!(replay.phase(), ReplayPhase::Playing);
        assert!(!replay.paused());
    }

    #[test]
    fn an_unfinished_take_is_listed_but_not_offered() {
        let episode_with = |frames, problem| {
            EpisodeInfo::new("episode_000001", "default")
                .with_arms(vec![EpisodeArm::new(
                    "follower",
                    Some("so101_follower".to_owned()),
                    vec!["shoulder_pan".to_owned()],
                )])
                .with_source("Teleoperation")
                .with_timeline(frames, 2.0)
                .with_sample_hz(60.0)
                .with_started_ms(1_770_000_000_000)
                .with_assessment(false, Some(3.5))
                .with_problem(problem)
        };
        let episode = episode_with(120, None);
        assert!(episode.usable());
        assert_eq!(
            episode.arm("follower").map(|arm| arm.joints().len()),
            Some(1)
        );
        assert_eq!(episode.arm("leader"), None);

        let episode = episode_with(120, Some("recording did not finish".to_owned()));
        assert!(!episode.usable(), "a truncated take must not be offered");

        // An empty take is unusable even without a recorded reason: there is
        // nothing in it to replay.
        let episode = episode_with(0, None);
        assert!(!episode.usable());
    }

    #[test]
    fn calibration_span_saturates_instead_of_overflowing() {
        let progress = CalibrationJointProgress::new("gripper", i64::MIN, 0).with_max(i64::MAX);
        assert_eq!(progress.span(), i64::MAX);
    }

    #[test]
    fn an_arms_condition_is_the_worst_of_its_motors() {
        assert_eq!(
            [
                MotorCondition::Nominal,
                MotorCondition::Fault,
                MotorCondition::Caution
            ]
            .into_iter()
            .max(),
            Some(MotorCondition::Fault)
        );
    }

    fn training_run(id: &str) -> TrainingRun {
        TrainingRun::starting(id, StartTrainingRequest::default())
            .with_source(TrainingSource::default())
    }

    fn training_metric(step: u64, train_loss: f64) -> TrainingMetricPoint {
        TrainingMetricPoint::new(step, train_loss)
    }

    #[test]
    fn incremental_training_updates_stay_bounded_without_dropping_early_history() {
        let mut snapshot =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![training_run("run-a")]);
        for step in 0..=12_000 {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(
                    TrainingMetricPoint::new(step, if step == 123 { 10_000.0 } else { 1.0 })
                        .with_validation_loss((step == 6_000).then_some(4.0))
                        .with_utilization((step == 777).then_some(9_000.0), None),
                ),
            });
        }
        for line in 0..=MAX_TRAINING_LOG_LINES {
            snapshot.apply(TrainingUpdate::Log {
                run_id: "run-a".to_owned(),
                line: format!("line {line}"),
            });
        }

        let run = &snapshot.runs()[0];
        assert!(run.metrics().len() <= MAX_TRAINING_METRIC_POINTS);
        assert_eq!(
            run.metrics().first().map(TrainingMetricPoint::step),
            Some(0)
        );
        assert_eq!(
            run.metrics().last().map(TrainingMetricPoint::step),
            Some(12_000)
        );
        assert!(run.metrics().iter().any(|point| point.step() == 123));
        assert!(run.metrics().iter().any(|point| point.step() == 777));
        assert!(run.metrics().iter().any(|point| point.step() == 6_000));
        assert_eq!(run.logs().len(), MAX_TRAINING_LOG_LINES);
        assert_eq!(run.logs()[0], "line 1");
        assert_eq!(run.current_step(), 12_000);
    }

    #[test]
    fn repeated_or_late_metric_updates_replace_the_step_without_regressing_progress() {
        let mut snapshot =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![training_run("run-a")]);
        for (step, loss) in [(2, 2.0), (1, 1.0), (2, 0.5)] {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(training_metric(step, loss)),
            });
        }

        let run = &snapshot.runs()[0];
        assert_eq!(run.current_step(), 2);
        assert_eq!(run.metrics().len(), 2);
        assert_eq!(run.metrics()[0].step(), 1);
        assert_eq!(run.metrics()[1].step(), 2);
        assert_eq!(run.metrics()[1].train_loss(), 0.5);
    }

    #[test]
    fn resolved_training_configuration_updates_the_run_snapshot() {
        let mut snapshot =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![training_run("run-a")]);
        let mut config = StartTrainingRequest::default();
        *config.act_mut() = ActTrainingConfig::default().with_camera_streams(vec![
            "follower.camera.gripper".to_owned(),
            "follower.camera.top".to_owned(),
        ]);
        snapshot.apply(TrainingUpdate::RunConfigured {
            run_id: "run-a".to_owned(),
            config: Box::new(config.clone()),
        });
        assert_eq!(snapshot.runs()[0].config(), &config);
    }

    #[test]
    fn removing_a_training_run_removes_its_complete_snapshot_record() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![
                TrainingRun::starting("run-a", StartTrainingRequest::default())
                    .with_source(TrainingSource::default())
                    .with_metrics(vec![TrainingMetricPoint::default()])
                    .with_checkpoints(vec![TrainingCheckpoint::default()])
                    .with_logs(vec!["retained only inside the run".to_owned()]),
                training_run("run-b"),
            ],
        );

        snapshot.apply(TrainingUpdate::RunRemoved {
            run_id: "run-a".to_owned(),
        });

        assert_eq!(snapshot.runs().len(), 1);
        assert_eq!(snapshot.runs()[0].id(), "run-b");
    }

    #[test]
    fn bootstrap_training_snapshot_replaces_the_placeholder_atomically() {
        let mut snapshot = TrainingSnapshot::default();
        let restored = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![training_run("restored-run")],
        );

        snapshot.apply(TrainingUpdate::Snapshot {
            snapshot: Box::new(restored.clone()),
        });

        assert_eq!(snapshot, restored);
    }
}
