//! Policy for exclusive ownership of robot motion.
//!
//! The hardware actor supplies plain admission facts, asks for a transition,
//! and executes the returned effects in order. This keeps the safety-critical
//! questions -- which arm may drive, which arm must be loose, and what failure
//! returns the product to Idle -- independent of motor drivers and transport.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlMode {
    Idle,
    Manual { robot: String },
    Teleoperate { leader: String, follower: String },
    Replay { robot: String, episode: String },
    Policy { robot: String, session: String },
}

impl ControlMode {
    fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Manual { .. } => "Manual",
            Self::Teleoperate { .. } => "Teleoperation",
            Self::Replay { .. } => "Replay",
            Self::Policy { .. } => "Policy",
        }
    }

    fn involves(&self, robot: &str) -> bool {
        match self {
            Self::Idle => false,
            Self::Manual { robot: owner }
            | Self::Replay { robot: owner, .. }
            | Self::Policy { robot: owner, .. } => owner == robot,
            Self::Teleoperate { leader, follower } => leader == robot || follower == robot,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CalibrationAdmission {
    Missing,
    Invalid(String),
    Ready(BTreeSet<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RobotAdmission {
    pub calibration: CalibrationAdmission,
    pub calibration_in_progress: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlState {
    pub mode: ControlMode,
    /// Connected robots only. Absence is the connection fact.
    pub robots: BTreeMap<String, RobotAdmission>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Intent {
    Select(ControlMode),
    Hold { robot: String },
    Release { robot: String },
    AuthorizeManualCommand { robot: String },
    AuthorizePolicyCommand { robot: String, session: String },
    StartCalibration { robot: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    ReplayLost,
    TeleoperationUnavailable,
    ReplayFinished,
    ReplayFailed { robot: String, error: String },
    RobotLost { robot: String, reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FailureAction {
    /// A partially applied ownership change is not a truthful control mode.
    /// Surrender ownership and remove queued motion before reporting it.
    AbandonTransition,
    /// Holding is a transaction: if torque cannot be established, surrender
    /// exclusive ownership and remove every queued motion target.
    AbandonHold { robot: String },
    /// Release is fail-safe: Idle is committed before torque-off is attempted,
    /// and a physical failure is reported without restoring the old owner.
    ReportRelease { robot: String },
    /// The caller adds operation-specific context; policy has already made any
    /// ownership change that must survive the physical failure.
    Propagate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Effect {
    ClearReplay,
    ResetAllTargets,
    ResetTargets {
        robot: String,
    },
    SetTorque {
        robot: String,
        engaged: bool,
        on_failure: FailureAction,
    },
    CommitMode(ControlMode),
    CommitModeWithError {
        mode: ControlMode,
        error: String,
    },
    SetActivity {
        active: bool,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Decision {
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionError {
    CalibrationInProgress(Vec<String>),
    NotConnected(String),
    MissingCalibration(String),
    InvalidCalibration { robot: String, reason: String },
    SameTeleoperationRobot,
    NoSharedCalibratedJoints { leader: String, follower: String },
    ManualCommandNotOwned(String),
    PolicyCommandNotOwned { robot: String, session: String },
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CalibrationInProgress(robots) => write!(
                formatter,
                "calibration is in progress for {}; finish or cancel it first",
                robots.join(", ")
            ),
            Self::NotConnected(robot) => write!(formatter, "robot {robot:?} is not connected"),
            Self::MissingCalibration(robot) => {
                write!(formatter, "robot {robot:?} must be calibrated first")
            }
            Self::InvalidCalibration { robot, reason } => write!(
                formatter,
                "robot {robot:?} has calibration that does not match its configured motors: {reason}"
            ),
            Self::SameTeleoperationRobot => {
                formatter.write_str("teleoperation needs two different robots")
            }
            Self::NoSharedCalibratedJoints { leader, follower } => write!(
                formatter,
                "robots {leader:?} and {follower:?} share no calibrated joints"
            ),
            Self::ManualCommandNotOwned(robot) => write!(
                formatter,
                "switch to manual mode for robot {robot:?} before commanding its joints"
            ),
            Self::PolicyCommandNotOwned { robot, session } => write!(
                formatter,
                "policy session {session:?} is not allowed to command robot {robot:?}"
            ),
        }
    }
}

/// Decide one complete control-session transition.
pub(crate) fn decide(state: &ControlState, intent: Intent) -> Result<Decision, AdmissionError> {
    match intent {
        Intent::Select(mode) => select(state, mode),
        Intent::Hold { robot } => {
            let mut decision = select(
                state,
                ControlMode::Manual {
                    robot: robot.clone(),
                },
            )?;
            decision.effects.push(Effect::SetTorque {
                robot: robot.clone(),
                engaged: true,
                on_failure: FailureAction::AbandonHold { robot },
            });
            Ok(decision)
        }
        Intent::Release { robot } => {
            let mut decision = select(state, ControlMode::Idle)?;
            decision.effects.push(Effect::SetTorque {
                robot: robot.clone(),
                engaged: false,
                on_failure: FailureAction::ReportRelease { robot },
            });
            Ok(decision)
        }
        Intent::AuthorizeManualCommand { robot } => {
            if matches!(&state.mode, ControlMode::Manual { robot: owner } if owner == &robot) {
                Ok(Decision {
                    effects: Vec::new(),
                })
            } else {
                Err(AdmissionError::ManualCommandNotOwned(robot))
            }
        }
        Intent::AuthorizePolicyCommand { robot, session } => {
            if matches!(
                &state.mode,
                ControlMode::Policy { robot: owner, session: active }
                    if owner == &robot && active == &session
            ) {
                Ok(Decision {
                    effects: Vec::new(),
                })
            } else {
                Err(AdmissionError::PolicyCommandNotOwned { robot, session })
            }
        }
        Intent::StartCalibration { robot } => Ok(start_calibration(state, robot)),
    }
}

/// Decide the ownership consequence of an observed outer-layer event.
pub(crate) fn decide_event(state: &ControlState, event: Event) -> Decision {
    match event {
        Event::ReplayLost => Decision {
            effects: vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: "the replay was lost".to_owned(),
                },
            ],
        },
        Event::TeleoperationUnavailable => Decision {
            effects: vec![Effect::SetActivity {
                active: false,
                error: Some("both arms must be calibrated".to_owned()),
            }],
        },
        Event::ReplayFinished => Decision {
            effects: vec![Effect::ClearReplay, Effect::CommitMode(ControlMode::Idle)],
        },
        Event::ReplayFailed { robot, error } => Decision {
            effects: vec![
                Effect::ClearReplay,
                Effect::ResetTargets { robot },
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error,
                },
            ],
        },
        Event::RobotLost { robot, reason } if state.mode.involves(&robot) => Decision {
            effects: vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: format!("stopped {}: {reason}", state.mode.label()),
                },
            ],
        },
        Event::RobotLost { .. } => Decision {
            effects: Vec::new(),
        },
    }
}

fn start_calibration(state: &ControlState, robot: String) -> Decision {
    let mut effects = Vec::new();
    if state.mode != ControlMode::Idle {
        effects.extend([
            Effect::ClearReplay,
            Effect::ResetAllTargets,
            Effect::CommitModeWithError {
                mode: ControlMode::Idle,
                error: format!("stopped {} to calibrate {robot}", state.mode.label()),
            },
        ]);
    }
    // Calibration is hand-guided, so verified release follows the ownership
    // surrender. A driver failure must never restore the previous owner.
    effects.push(Effect::SetTorque {
        robot,
        engaged: false,
        on_failure: FailureAction::Propagate,
    });
    Decision { effects }
}

fn select(state: &ControlState, mode: ControlMode) -> Result<Decision, AdmissionError> {
    let sweeping = state
        .robots
        .iter()
        .filter(|(_, robot)| robot.calibration_in_progress)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if !sweeping.is_empty() && mode != ControlMode::Idle {
        return Err(AdmissionError::CalibrationInProgress(sweeping));
    }

    match &mode {
        ControlMode::Idle => {}
        ControlMode::Manual { robot }
        | ControlMode::Replay { robot, .. }
        | ControlMode::Policy { robot, .. } => {
            ready_joints(state, robot)?;
        }
        ControlMode::Teleoperate { leader, follower } => {
            if leader == follower {
                return Err(AdmissionError::SameTeleoperationRobot);
            }
            let leader_joints = ready_joints(state, leader)?;
            let follower_joints = ready_joints(state, follower)?;
            if leader_joints.is_disjoint(follower_joints) {
                return Err(AdmissionError::NoSharedCalibratedJoints {
                    leader: leader.clone(),
                    follower: follower.clone(),
                });
            }
        }
    }

    let mut effects = Vec::new();
    // A replay plan belongs to one admitted ownership transaction. Even a
    // replay-to-replay replacement drops the old plan first, so a failed
    // replacement can never resume stale motion.
    effects.push(Effect::ClearReplay);
    effects.push(Effect::ResetAllTargets);

    match &mode {
        ControlMode::Teleoperate { leader, follower } => {
            // Fail loose: free the hand-driven arm before energizing the arm
            // that follows it.
            effects.push(Effect::SetTorque {
                robot: leader.clone(),
                engaged: false,
                on_failure: FailureAction::AbandonTransition,
            });
            effects.push(Effect::SetTorque {
                robot: follower.clone(),
                engaged: true,
                on_failure: FailureAction::AbandonTransition,
            });
        }
        ControlMode::Replay { robot, .. } | ControlMode::Policy { robot, .. } => {
            effects.push(Effect::SetTorque {
                robot: robot.clone(),
                engaged: true,
                on_failure: FailureAction::AbandonTransition,
            });
        }
        ControlMode::Idle | ControlMode::Manual { .. } => {}
    }
    // Ownership changes only after all required physical transitions succeed.
    effects.push(Effect::CommitMode(mode));
    Ok(Decision { effects })
}

fn ready_joints<'a>(
    state: &'a ControlState,
    robot: &str,
) -> Result<&'a BTreeSet<String>, AdmissionError> {
    let robot_facts = state
        .robots
        .get(robot)
        .ok_or_else(|| AdmissionError::NotConnected(robot.to_owned()))?;
    match &robot_facts.calibration {
        CalibrationAdmission::Ready(joints) => Ok(joints),
        CalibrationAdmission::Missing => Err(AdmissionError::MissingCalibration(robot.to_owned())),
        CalibrationAdmission::Invalid(reason) => Err(AdmissionError::InvalidCalibration {
            robot: robot.to_owned(),
            reason: reason.clone(),
        }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FailureDecision {
    pub effects: Vec<Effect>,
    pub error: String,
}

/// Decide the state consequence of a physical effect failure.
pub(crate) fn effect_failed(action: FailureAction, cause: String) -> FailureDecision {
    match action {
        FailureAction::AbandonTransition => FailureDecision {
            effects: idle_effects(),
            error: format!(
                "could not establish the requested control mode: {cause}; returned control to Idle"
            ),
        },
        FailureAction::AbandonHold { robot } => FailureDecision {
            effects: idle_effects(),
            error: format!(
                "could not engage holding torque for robot {robot:?}: {cause}; returned control to Idle"
            ),
        },
        FailureAction::ReportRelease { robot } => FailureDecision {
            effects: Vec::new(),
            error: format!("entered Idle but could not release robot {robot:?}: {cause}"),
        },
        FailureAction::Propagate => FailureDecision {
            effects: Vec::new(),
            error: cause,
        },
    }
}

fn idle_effects() -> Vec<Effect> {
    vec![
        Effect::ClearReplay,
        Effect::ResetAllTargets,
        Effect::CommitMode(ControlMode::Idle),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayState {
    Seeking,
    Playing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReplayEvent {
    Paused,
    SeekObserved { arrived: bool },
    PlaybackObserved { finished: bool },
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReplayDecision {
    Keep,
    EnterPlaying,
    FinishToIdle,
    FailToIdle(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayTransitionError {
    PlaybackObservedWhileSeeking,
    SeekObservedWhilePlaying,
}

/// Own the legal replay lifecycle; clocks, positions, and target conversion
/// remain observations supplied by the hardware adapter.
pub(crate) fn decide_replay(
    state: ReplayState,
    event: ReplayEvent,
) -> Result<ReplayDecision, ReplayTransitionError> {
    match (state, event) {
        (_, ReplayEvent::Paused) => Ok(ReplayDecision::Keep),
        (_, ReplayEvent::Failed(error)) => Ok(ReplayDecision::FailToIdle(error)),
        (ReplayState::Seeking, ReplayEvent::SeekObserved { arrived: true }) => {
            Ok(ReplayDecision::EnterPlaying)
        }
        (ReplayState::Seeking, ReplayEvent::SeekObserved { arrived: false }) => {
            Ok(ReplayDecision::Keep)
        }
        (ReplayState::Playing, ReplayEvent::PlaybackObserved { finished: true }) => {
            Ok(ReplayDecision::FinishToIdle)
        }
        (ReplayState::Playing, ReplayEvent::PlaybackObserved { finished: false }) => {
            Ok(ReplayDecision::Keep)
        }
        (ReplayState::Seeking, ReplayEvent::PlaybackObserved { .. }) => {
            Err(ReplayTransitionError::PlaybackObservedWhileSeeking)
        }
        (ReplayState::Playing, ReplayEvent::SeekObserved { .. }) => {
            Err(ReplayTransitionError::SeekObservedWhilePlaying)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(joints: &[&str]) -> RobotAdmission {
        RobotAdmission {
            calibration: CalibrationAdmission::Ready(
                joints.iter().map(|joint| (*joint).to_owned()).collect(),
            ),
            calibration_in_progress: false,
        }
    }

    fn state() -> ControlState {
        ControlState {
            mode: ControlMode::Idle,
            robots: BTreeMap::from([
                ("leader".to_owned(), ready(&["shoulder", "elbow"])),
                ("follower".to_owned(), ready(&["elbow", "wrist"])),
                ("other".to_owned(), ready(&["gripper"])),
            ]),
        }
    }

    #[test]
    fn idle_clears_replay_resets_targets_then_commits() {
        let decision = decide(&state(), Intent::Select(ControlMode::Idle)).unwrap();
        assert_eq!(decision.effects, idle_effects());
    }

    #[test]
    fn manual_claims_one_calibrated_robot_without_implicit_torque() {
        let mode = ControlMode::Manual {
            robot: "leader".to_owned(),
        };
        let decision = decide(&state(), Intent::Select(mode.clone())).unwrap();
        assert_eq!(
            decision.effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitMode(mode)
            ]
        );
    }

    #[test]
    fn hold_commits_manual_before_engaging_and_rolls_back_on_failure() {
        let decision = decide(
            &state(),
            Intent::Hold {
                robot: "leader".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            decision.effects.last(),
            Some(&Effect::SetTorque {
                robot: "leader".to_owned(),
                engaged: true,
                on_failure: FailureAction::AbandonHold {
                    robot: "leader".to_owned()
                }
            })
        );
        let recovery = effect_failed(
            FailureAction::AbandonHold {
                robot: "leader".to_owned(),
            },
            "driver refused".to_owned(),
        );
        assert_eq!(recovery.effects, idle_effects());
        assert!(recovery.error.contains("returned control to Idle"));
    }

    #[test]
    fn release_enters_idle_before_attempting_torque_off() {
        let decision = decide(
            &state(),
            Intent::Release {
                robot: "leader".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(&decision.effects[..3], idle_effects());
        assert_eq!(
            decision.effects[3],
            Effect::SetTorque {
                robot: "leader".to_owned(),
                engaged: false,
                on_failure: FailureAction::ReportRelease {
                    robot: "leader".to_owned()
                }
            }
        );
    }

    #[test]
    fn teleoperation_releases_leader_before_engaging_follower_and_committing() {
        let mode = ControlMode::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        };
        let decision = decide(&state(), Intent::Select(mode.clone())).unwrap();
        assert_eq!(
            decision.effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::SetTorque {
                    robot: "leader".to_owned(),
                    engaged: false,
                    on_failure: FailureAction::AbandonTransition,
                },
                Effect::SetTorque {
                    robot: "follower".to_owned(),
                    engaged: true,
                    on_failure: FailureAction::AbandonTransition,
                },
                Effect::CommitMode(mode),
            ]
        );
    }

    #[test]
    fn replay_and_policy_engage_their_exclusive_owner_before_commit() {
        for mode in [
            ControlMode::Replay {
                robot: "leader".to_owned(),
                episode: "take".to_owned(),
            },
            ControlMode::Policy {
                robot: "leader".to_owned(),
                session: "run".to_owned(),
            },
        ] {
            let decision = decide(&state(), Intent::Select(mode.clone())).unwrap();
            assert_eq!(decision.effects.last(), Some(&Effect::CommitMode(mode)));
            assert!(decision.effects.iter().any(|effect| matches!(
                effect,
                Effect::SetTorque { robot, engaged: true, .. } if robot == "leader"
            )));
        }
    }

    #[test]
    fn replay_replacement_discards_the_old_plan_before_physical_transition() {
        let mode = ControlMode::Replay {
            robot: "leader".to_owned(),
            episode: "replacement".to_owned(),
        };
        let decision = decide(&state(), Intent::Select(mode)).unwrap();
        assert_eq!(decision.effects.first(), Some(&Effect::ClearReplay));
    }

    #[test]
    fn a_calibration_sweep_blocks_every_non_idle_owner() {
        let mut state = state();
        state
            .robots
            .get_mut("leader")
            .unwrap()
            .calibration_in_progress = true;
        for mode in [
            ControlMode::Manual {
                robot: "leader".to_owned(),
            },
            ControlMode::Replay {
                robot: "leader".to_owned(),
                episode: "take".to_owned(),
            },
            ControlMode::Policy {
                robot: "leader".to_owned(),
                session: "run".to_owned(),
            },
        ] {
            assert!(matches!(
                decide(&state, Intent::Select(mode)),
                Err(AdmissionError::CalibrationInProgress(_))
            ));
        }
        assert!(decide(&state, Intent::Select(ControlMode::Idle)).is_ok());
    }

    #[test]
    fn connection_and_calibration_admission_fail_closed() {
        let mut state = state();
        assert_eq!(
            decide(
                &state,
                Intent::Select(ControlMode::Manual {
                    robot: "absent".to_owned()
                })
            ),
            Err(AdmissionError::NotConnected("absent".to_owned()))
        );
        state.robots.get_mut("leader").unwrap().calibration = CalibrationAdmission::Missing;
        assert!(matches!(
            decide(
                &state,
                Intent::Select(ControlMode::Manual {
                    robot: "leader".to_owned()
                })
            ),
            Err(AdmissionError::MissingCalibration(_))
        ));
        state.robots.get_mut("leader").unwrap().calibration =
            CalibrationAdmission::Invalid("motor mismatch".to_owned());
        assert!(matches!(
            decide(
                &state,
                Intent::Select(ControlMode::Manual {
                    robot: "leader".to_owned()
                })
            ),
            Err(AdmissionError::InvalidCalibration { .. })
        ));
    }

    #[test]
    fn teleoperation_requires_distinct_robots_with_a_shared_joint() {
        assert_eq!(
            decide(
                &state(),
                Intent::Select(ControlMode::Teleoperate {
                    leader: "leader".to_owned(),
                    follower: "leader".to_owned(),
                })
            ),
            Err(AdmissionError::SameTeleoperationRobot)
        );
        assert!(matches!(
            decide(
                &state(),
                Intent::Select(ControlMode::Teleoperate {
                    leader: "leader".to_owned(),
                    follower: "other".to_owned(),
                })
            ),
            Err(AdmissionError::NoSharedCalibratedJoints { .. })
        ));
    }

    #[test]
    fn command_authorization_requires_the_exact_owner_and_session() {
        let mut state = state();
        state.mode = ControlMode::Manual {
            robot: "leader".to_owned(),
        };
        assert_eq!(
            decide(
                &state,
                Intent::AuthorizeManualCommand {
                    robot: "leader".to_owned()
                }
            ),
            Ok(Decision {
                effects: Vec::new()
            })
        );
        assert_eq!(
            decide(
                &state,
                Intent::AuthorizeManualCommand {
                    robot: "follower".to_owned()
                }
            ),
            Err(AdmissionError::ManualCommandNotOwned("follower".to_owned()))
        );

        state.mode = ControlMode::Policy {
            robot: "leader".to_owned(),
            session: "run-1".to_owned(),
        };
        assert_eq!(
            decide(
                &state,
                Intent::AuthorizePolicyCommand {
                    robot: "leader".to_owned(),
                    session: "run-1".to_owned(),
                }
            ),
            Ok(Decision {
                effects: Vec::new()
            })
        );
        for (robot, session) in [("follower", "run-1"), ("leader", "run-2")] {
            assert!(matches!(
                decide(
                    &state,
                    Intent::AuthorizePolicyCommand {
                        robot: robot.to_owned(),
                        session: session.to_owned(),
                    }
                ),
                Err(AdmissionError::PolicyCommandNotOwned { .. })
            ));
        }
    }

    #[test]
    fn calibration_surrenders_ownership_before_requesting_release() {
        let mut state = state();
        state.mode = ControlMode::Replay {
            robot: "leader".to_owned(),
            episode: "take".to_owned(),
        };
        assert_eq!(
            decide(
                &state,
                Intent::StartCalibration {
                    robot: "leader".to_owned()
                }
            )
            .unwrap()
            .effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: "stopped Replay to calibrate leader".to_owned(),
                },
                Effect::SetTorque {
                    robot: "leader".to_owned(),
                    engaged: false,
                    on_failure: FailureAction::Propagate,
                },
            ]
        );

        state.mode = ControlMode::Idle;
        assert_eq!(
            decide(
                &state,
                Intent::StartCalibration {
                    robot: "leader".to_owned()
                }
            )
            .unwrap()
            .effects,
            vec![Effect::SetTorque {
                robot: "leader".to_owned(),
                engaged: false,
                on_failure: FailureAction::Propagate,
            }]
        );
        assert_eq!(
            effect_failed(FailureAction::Propagate, "driver refused".to_owned()),
            FailureDecision {
                effects: Vec::new(),
                error: "driver refused".to_owned(),
            }
        );
    }

    #[test]
    fn losing_an_owned_robot_clears_motion_and_reports_the_previous_mode() {
        let mut state = state();
        state.mode = ControlMode::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        };
        assert_eq!(
            decide_event(
                &state,
                Event::RobotLost {
                    robot: "leader".to_owned(),
                    reason: "its bus stopped responding".to_owned(),
                }
            )
            .effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: "stopped Teleoperation: its bus stopped responding".to_owned(),
                },
            ]
        );
        assert!(
            decide_event(
                &state,
                Event::RobotLost {
                    robot: "other".to_owned(),
                    reason: "gone".to_owned(),
                }
            )
            .effects
            .is_empty()
        );
    }

    #[test]
    fn runtime_observations_have_explicit_ownership_and_target_effects() {
        let state = state();
        assert_eq!(
            decide_event(&state, Event::ReplayLost).effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetAllTargets,
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: "the replay was lost".to_owned(),
                },
            ]
        );
        assert_eq!(
            decide_event(&state, Event::TeleoperationUnavailable).effects,
            vec![Effect::SetActivity {
                active: false,
                error: Some("both arms must be calibrated".to_owned()),
            }]
        );
        assert_eq!(
            decide_event(&state, Event::ReplayFinished).effects,
            vec![Effect::ClearReplay, Effect::CommitMode(ControlMode::Idle)]
        );
        assert_eq!(
            decide_event(
                &state,
                Event::ReplayFailed {
                    robot: "leader".to_owned(),
                    error: "unsafe trajectory".to_owned(),
                }
            )
            .effects,
            vec![
                Effect::ClearReplay,
                Effect::ResetTargets {
                    robot: "leader".to_owned()
                },
                Effect::CommitModeWithError {
                    mode: ControlMode::Idle,
                    error: "unsafe trajectory".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn replay_lifecycle_has_only_legal_phase_transitions() {
        assert_eq!(
            decide_replay(
                ReplayState::Seeking,
                ReplayEvent::SeekObserved { arrived: false }
            ),
            Ok(ReplayDecision::Keep)
        );
        assert_eq!(
            decide_replay(
                ReplayState::Seeking,
                ReplayEvent::SeekObserved { arrived: true }
            ),
            Ok(ReplayDecision::EnterPlaying)
        );
        assert_eq!(
            decide_replay(
                ReplayState::Playing,
                ReplayEvent::PlaybackObserved { finished: false }
            ),
            Ok(ReplayDecision::Keep)
        );
        assert_eq!(
            decide_replay(
                ReplayState::Playing,
                ReplayEvent::PlaybackObserved { finished: true }
            ),
            Ok(ReplayDecision::FinishToIdle)
        );
        assert_eq!(
            decide_replay(
                ReplayState::Seeking,
                ReplayEvent::PlaybackObserved { finished: true }
            ),
            Err(ReplayTransitionError::PlaybackObservedWhileSeeking)
        );
        assert_eq!(
            decide_replay(
                ReplayState::Playing,
                ReplayEvent::SeekObserved { arrived: true }
            ),
            Err(ReplayTransitionError::SeekObservedWhilePlaying)
        );
        for state in [ReplayState::Seeking, ReplayState::Playing] {
            assert_eq!(
                decide_replay(state, ReplayEvent::Paused),
                Ok(ReplayDecision::Keep)
            );
            assert_eq!(
                decide_replay(state, ReplayEvent::Failed("fault".to_owned())),
                Ok(ReplayDecision::FailToIdle("fault".to_owned()))
            );
        }
    }
}
