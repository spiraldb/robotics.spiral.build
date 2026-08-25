//! Studio bootstrap status, top-level snapshot, and request failures.

use serde::{Deserialize, Serialize};
use spiral_robotics_core::operation::{
    RobotReadiness, TeleoperationCandidate, TeleoperationCandidates,
};

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
    pub readiness: BackendReadiness,
    /// Human-readable name of the active persistent store.
    pub storage: String,
    /// Every robot configuration found in SpiralDB, by ID.
    pub robots: Vec<RobotSummary>,
    /// Cameras the server has discovered.
    pub cameras: Vec<CameraStatus>,
    /// The currently selected operating mode.
    pub mode: ModeStatus,
    /// Problems worth showing the operator: invalid robot configurations, cameras
    /// that failed to probe, and similar non-fatal trouble.
    pub issues: Vec<String>,
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
    pub state: BackendReadinessState,
    /// True only when catalog-dependent requests may safely be admitted.
    pub catalog_available: bool,
    /// Short operator-facing description of the current phase or problem.
    pub detail: String,
}

impl Default for BackendReadiness {
    fn default() -> Self {
        Self {
            state: BackendReadinessState::Bootstrapping,
            catalog_available: false,
            detail: "Starting Studio services".to_owned(),
        }
    }
}

impl StudioSnapshot {
    /// Pick the best connected pair for a recording-led teleoperation session.
    ///
    /// Robot type is a preference rather than a requirement: a configured leader
    /// and follower win, while two otherwise ready compatible arms still work.
    /// Compatibility is checked here so the Record button and the server make the
    /// same decision instead of offering a pair the control thread must reject.
    pub fn default_teleoperation_pair(&self) -> Option<(String, String)> {
        let candidates = self
            .robots
            .iter()
            .map(|robot| {
                TeleoperationCandidate::new(
                    robot.id.clone(),
                    RobotReadiness::new(
                        robot.connection.connected,
                        robot.calibrated,
                        robot.problem.is_none(),
                    ),
                    robot
                        .motors
                        .iter()
                        .map(|motor| motor.name.clone())
                        .collect(),
                )
                .with_robot_type(robot.robot_type.clone())
            })
            .collect::<Vec<_>>();
        TeleoperationCandidates::from(candidates.as_slice()).best_pair()
    }
}

/// A failed request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
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
    use spiral_robotics_core::calibration::{MotorCalibration, MotorDirection, MotorReading};

    #[test]
    fn act_defaults_match_current_lerobot_architecture() {
        let config = ActTrainingConfig::default();
        assert_eq!(config.hidden_dim, 512);
        assert_eq!(config.feedforward_dim, 3_200);
        assert_eq!(config.vae_encoder_layers, 4);
        assert_eq!(config.decoder_layers, 1);
        assert!(config.camera_streams.is_empty());
        let request = StartTrainingRequest::default();
        assert_eq!(request.batch_size, 8);
        assert_eq!(request.steps, 100_000);
        assert_eq!(request.checkpoint_every, 20_000);
        assert_eq!(request.seed, 1_000);
        assert_eq!(request.validation_split, 0.1);
    }

    #[test]
    fn historical_training_requests_keep_no_validation_semantics() {
        let mut serialized_request = serde_json::to_value(StartTrainingRequest::default()).unwrap();
        serialized_request
            .as_object_mut()
            .unwrap()
            .remove("validation_split");
        let request: StartTrainingRequest = serde_json::from_value(serialized_request).unwrap();
        assert_eq!(request.validation_split, 0.0);
    }

    #[test]
    fn older_rollout_requests_receive_the_safe_default_duration() {
        let request: StartRolloutRequest = serde_json::from_value(serde_json::json!({
            "run_id": "run-1",
            "checkpoint_step": 20_000,
            "robot": "follower"
        }))
        .expect("duration defaults for an older client");

        assert_eq!(request.duration_seconds, DEFAULT_ROLLOUT_DURATION_SECONDS);
    }

    #[test]
    fn motor_summary_adapts_domain_calibration_at_the_contract_edge() {
        let summary = MotorSummary {
            name: "joint".to_owned(),
            id: 7,
            direction: MotorDirection::Reversed,
            model: Some("sts3215".to_owned()),
            calibration: Some(
                MotorCalibration::new(
                    7,
                    MotorDirection::Reversed,
                    MotorReading::new(200, 100, 300),
                )
                .unwrap(),
            ),
            limits: MotorLimits::default(),
        };

        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["direction"], "reversed");
        assert_eq!(json["calibration"]["direction"], "reversed");
        assert_eq!(
            serde_json::from_value::<MotorSummary>(json).unwrap(),
            summary
        );
    }

    fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
        RobotSummary {
            id: id.to_owned(),
            robot_type: robot_type.map(str::to_owned),
            port: None,
            motors: vec![MotorSummary {
                name: "shoulder".to_owned(),
                id: 1,
                direction: MotorDirection::Normal,
                model: None,
                calibration: None,
                limits: MotorLimits::default(),
            }],
            cameras: Vec::new(),
            calibrated: true,
            problem: None,
            connection: ConnectionStatus {
                connected: true,
                error: None,
            },
        }
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
        let mut disconnected = robot("disconnected-leader", Some("leader"));
        disconnected.connection.connected = false;
        let mut incompatible = robot("incompatible-leader", Some("leader"));
        incompatible.motors[0].name = "elbow".to_owned();
        let follower = robot("follower", Some("follower"));
        let fallback = robot("fallback", None);
        let studio = StudioSnapshot {
            robots: vec![disconnected, incompatible, follower, fallback.clone()],
            ..StudioSnapshot::default()
        };

        assert_eq!(
            studio.default_teleoperation_pair(),
            Some((fallback.id, "follower".to_owned()))
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
        assert_eq!(created.id, "arm");
        assert_eq!(created.robot_type, None);

        let update: UpdateRobotRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(update, UpdateRobotRequest::default());

        let settings: UpdateDataSettingsRequest = serde_json::from_str(
            r#"{
                "video": { "enabled": true }
            }"#,
        )
        .unwrap();
        assert_eq!(settings.spiral_endpoint, DEFAULT_SPIRAL_ENDPOINT);
        assert_eq!(settings.spiral_spfs_endpoint, DEFAULT_SPIRAL_SPFS_ENDPOINT);
        assert_eq!(settings.training, TrainingRuntimeSettings::default());
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

        assert_eq!(replay.purpose, ReplayPurpose::Timeline);
        assert_eq!(replay.phase, ReplayPhase::Playing);
        assert!(!replay.paused);
    }

    #[test]
    fn an_unfinished_take_is_listed_but_not_offered() {
        let mut episode = EpisodeSummary {
            id: "episode_000001".to_owned(),
            dataset_id: "default".to_owned(),
            arms: vec![EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                joints: vec!["shoulder_pan".to_owned()],
            }],
            frames: 120,
            duration_seconds: 2.0,
            recording_gaps: Vec::new(),
            sample_hz: 60.0,
            started_ms: 1_770_000_000_000,
            source: "Teleoperation".to_owned(),
            max_follow_error_degrees: Some(3.5),
            discarded: false,
            problem: None,
        };
        assert!(episode.usable());
        assert_eq!(episode.arm("follower").map(|arm| arm.joints.len()), Some(1));
        assert_eq!(episode.arm("leader"), None);

        episode.problem = Some("recording did not finish".to_owned());
        assert!(!episode.usable(), "a truncated take must not be offered");

        // An empty take is unusable even without a recorded reason: there is
        // nothing in it to replay.
        episode.problem = None;
        episode.frames = 0;
        assert!(!episode.usable());
    }

    #[test]
    fn calibration_span_saturates_instead_of_overflowing() {
        let progress = CalibrationJointProgress {
            name: "gripper".to_owned(),
            min: i64::MIN,
            position: 0,
            max: i64::MAX,
        };
        assert_eq!(progress.span(), i64::MAX);
    }

    fn joint() -> JointTelemetry {
        JointTelemetry {
            name: "shoulder_pan".to_owned(),
            id: 1,
            raw: 2048,
            degrees: None,
            normalized: None,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
            temperature_celsius: Some(30.0),
            voltage_volts: Some(7.4),
            current_amps: Some(0.2),
            load: Some(0.1),
            speed_degrees_per_second: Some(0.0),
            temperature_rise_per_minute: Some(0.0),
            faults: Vec::new(),
        }
    }

    fn limits() -> MotorLimits {
        MotorLimits {
            max_temperature_celsius: Some(70.0),
            min_voltage_volts: Some(4.5),
            max_voltage_volts: Some(9.0),
            max_load: Some(1.0),
        }
    }

    #[test]
    fn a_motor_within_its_own_limits_is_nominal() {
        assert_eq!(joint().condition(&limits()), MotorCondition::Nominal);
    }

    #[test]
    fn nearing_a_limit_is_caution_and_the_motors_own_verdict_is_fault() {
        let hot = JointTelemetry {
            temperature_celsius: Some(70.0 * CAUTION_LIMIT_FRACTION),
            ..joint()
        };
        assert_eq!(hot.condition(&limits()), MotorCondition::Caution);

        let strained = JointTelemetry {
            load: Some(-0.9),
            ..joint()
        };
        assert_eq!(
            strained.condition(&limits()),
            MotorCondition::Caution,
            "load is judged by how hard it is pushing, not which way"
        );

        let sagging = JointTelemetry {
            voltage_volts: Some(4.5 + CAUTION_VOLTAGE_MARGIN_VOLTS - 0.1),
            ..joint()
        };
        assert_eq!(sagging.condition(&limits()), MotorCondition::Caution);

        // The motor's own fault byte outranks every margin applied here.
        let faulted = JointTelemetry {
            faults: vec![MotorFault::Overheat],
            ..joint()
        };
        assert_eq!(faulted.condition(&limits()), MotorCondition::Fault);
    }

    #[test]
    fn without_limits_only_the_motors_own_faults_can_speak() {
        let scorching = JointTelemetry {
            temperature_celsius: Some(200.0),
            ..joint()
        };

        assert_eq!(
            scorching.condition(&MotorLimits::default()),
            MotorCondition::Nominal,
            "there is nothing to be near, and a threshold invented here would be a guess"
        );
        assert_eq!(
            JointTelemetry {
                faults: vec![MotorFault::Unrecognized],
                ..scorching
            }
            .condition(&MotorLimits::default()),
            MotorCondition::Fault
        );
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

    #[test]
    fn incremental_training_updates_stay_bounded_without_dropping_early_history() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        for step in 0..=12_000 {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingMetricPoint {
                    step,
                    train_loss: if step == 123 { 10_000.0 } else { 1.0 },
                    validation_loss: (step == 6_000).then_some(4.0),
                    cpu_percent: (step == 777).then_some(9_000.0),
                    ..TrainingMetricPoint::default()
                }),
            });
        }
        for line in 0..=MAX_TRAINING_LOG_LINES {
            snapshot.apply(TrainingUpdate::Log {
                run_id: "run-a".to_owned(),
                line: format!("line {line}"),
            });
        }

        let run = &snapshot.runs[0];
        assert!(run.metrics.len() <= MAX_TRAINING_METRIC_POINTS);
        assert_eq!(run.metrics.first().map(|point| point.step), Some(0));
        assert_eq!(run.metrics.last().map(|point| point.step), Some(12_000));
        assert!(run.metrics.iter().any(|point| point.step == 123));
        assert!(run.metrics.iter().any(|point| point.step == 777));
        assert!(run.metrics.iter().any(|point| point.step == 6_000));
        assert_eq!(run.logs.len(), MAX_TRAINING_LOG_LINES);
        assert_eq!(run.logs[0], "line 1");
        assert_eq!(run.current_step, 12_000);
    }

    #[test]
    fn repeated_or_late_metric_updates_replace_the_step_without_regressing_progress() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        for (step, loss) in [(2, 2.0), (1, 1.0), (2, 0.5)] {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingMetricPoint {
                    step,
                    train_loss: loss,
                    ..TrainingMetricPoint::default()
                }),
            });
        }

        let run = &snapshot.runs[0];
        assert_eq!(run.current_step, 2);
        assert_eq!(run.metrics.len(), 2);
        assert_eq!(run.metrics[0].step, 1);
        assert_eq!(run.metrics[1].step, 2);
        assert_eq!(run.metrics[1].train_loss, 0.5);
    }

    #[test]
    fn resolved_training_configuration_updates_the_run_snapshot() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        let mut config = StartTrainingRequest::default();
        config.act.camera_streams = vec![
            "follower.camera.gripper".to_owned(),
            "follower.camera.top".to_owned(),
        ];
        snapshot.apply(TrainingUpdate::RunConfigured {
            run_id: "run-a".to_owned(),
            config: Box::new(config.clone()),
        });
        assert_eq!(snapshot.runs[0].config, config);
    }

    #[test]
    fn removing_a_training_run_removes_its_complete_snapshot_record() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![
                TrainingRun {
                    id: "run-a".to_owned(),
                    metrics: vec![TrainingMetricPoint::default()],
                    checkpoints: vec![TrainingCheckpoint::default()],
                    logs: vec!["retained only inside the run".to_owned()],
                    ..TrainingRun::default()
                },
                TrainingRun {
                    id: "run-b".to_owned(),
                    ..TrainingRun::default()
                },
            ],
            ..TrainingSnapshot::default()
        };

        snapshot.apply(TrainingUpdate::RunRemoved {
            run_id: "run-a".to_owned(),
        });

        assert_eq!(snapshot.runs.len(), 1);
        assert_eq!(snapshot.runs[0].id, "run-b");
    }

    #[test]
    fn bootstrap_training_snapshot_replaces_the_placeholder_atomically() {
        let mut snapshot = TrainingSnapshot::default();
        let restored = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "restored-run".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };

        snapshot.apply(TrainingUpdate::Snapshot {
            snapshot: Box::new(restored.clone()),
        });

        assert_eq!(snapshot, restored);
    }
}
