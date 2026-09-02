//! Atomic operator actions for changing how a robot is held.

use std::collections::BTreeMap;

use spiral_robotics_hardware::NormalizedPosition;

use crate::OperationError;

/// Consequential robot-control actions shared by every delivery mechanism.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RobotControlCommand {
    Hold {
        robot: String,
    },
    Release {
        robot: String,
    },
    MoveJoints {
        robot: String,
        normalized: BTreeMap<String, f64>,
    },
}

/// The capability-owned boundary implemented by a robot-control adapter.
pub(crate) trait RobotControlPort {
    async fn hold(&self, robot: &str) -> Result<(), String>;
    async fn release(&self, robot: &str) -> Result<(), String>;
    async fn set_joint_goals(
        &self,
        robot: &str,
        normalized: BTreeMap<String, NormalizedPosition>,
    ) -> Result<(), String>;
}

pub(crate) struct RobotControlActions<'a, P: RobotControlPort + ?Sized> {
    control: &'a P,
}

impl<'a, P: RobotControlPort + ?Sized> RobotControlActions<'a, P> {
    pub(crate) const fn new(control: &'a P) -> Self {
        Self { control }
    }

    /// Execute operator-facing robot-control actions as one control-thread command.
    pub(crate) async fn execute(&self, command: RobotControlCommand) -> Result<(), OperationError> {
        let result = match command {
            RobotControlCommand::Hold { robot } => self.control.hold(&robot).await,
            RobotControlCommand::Release { robot } => self.control.release(&robot).await,
            RobotControlCommand::MoveJoints { robot, normalized } => {
                if normalized.is_empty() {
                    return Err(OperationError::InvalidInput(
                        "no joints were given a target".to_owned(),
                    ));
                }
                let normalized = normalized
                    .into_iter()
                    .map(|(joint, value)| {
                        NormalizedPosition::new(value)
                            .map(|value| (joint.clone(), value))
                            .map_err(|error| {
                                OperationError::InvalidInput(format!(
                                    "joint {joint:?} has an invalid target: {error}"
                                ))
                            })
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;
                self.control.set_joint_goals(&robot, normalized).await
            }
        };
        result.map_err(OperationError::conflict)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeControl {
        calls: Mutex<Vec<String>>,
        failure: Option<String>,
    }

    impl RobotControlPort for FakeControl {
        async fn hold(&self, robot: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("hold:{robot}"));
            self.failure.clone().map_or(Ok(()), Err)
        }

        async fn release(&self, robot: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("release:{robot}"));
            self.failure.clone().map_or(Ok(()), Err)
        }

        async fn set_joint_goals(
            &self,
            robot: &str,
            _normalized: BTreeMap<String, NormalizedPosition>,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("move:{robot}"));
            self.failure.clone().map_or(Ok(()), Err)
        }
    }

    #[tokio::test]
    async fn dispatches_one_semantic_command_to_the_port() {
        let control = FakeControl::default();
        RobotControlActions::new(&control)
            .execute(RobotControlCommand::Hold {
                robot: "leader".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(*control.calls.lock().unwrap(), ["hold:leader"]);
    }

    #[tokio::test]
    async fn adapter_refusals_are_transport_neutral_conflicts() {
        let control = FakeControl {
            failure: Some("robot is disconnected".to_owned()),
            ..FakeControl::default()
        };
        let error = RobotControlActions::new(&control)
            .execute(RobotControlCommand::Release {
                robot: "follower".to_owned(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error,
            OperationError::Conflict("robot is disconnected".to_owned())
        );
    }

    #[tokio::test]
    async fn malformed_joint_goals_never_reach_the_adapter() {
        let control = FakeControl::default();
        for normalized in [
            BTreeMap::new(),
            BTreeMap::from([("shoulder".to_owned(), f64::NAN)]),
            BTreeMap::from([("shoulder".to_owned(), f64::INFINITY)]),
            BTreeMap::from([("shoulder".to_owned(), f64::from_bits(1.0_f64.to_bits() + 1))]),
            BTreeMap::from([(
                "shoulder".to_owned(),
                f64::from_bits((-1.0_f64).to_bits() + 1),
            )]),
        ] {
            let error = RobotControlActions::new(&control)
                .execute(RobotControlCommand::MoveJoints {
                    robot: "arm".to_owned(),
                    normalized,
                })
                .await
                .unwrap_err();
            assert!(matches!(error, OperationError::InvalidInput(_)));
        }
        assert!(control.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_normalized_boundaries_reach_the_adapter() {
        let control = FakeControl::default();
        for value in [-1.0, 1.0] {
            RobotControlActions::new(&control)
                .execute(RobotControlCommand::MoveJoints {
                    robot: "arm".to_owned(),
                    normalized: BTreeMap::from([("shoulder".to_owned(), value)]),
                })
                .await
                .unwrap();
        }
        assert_eq!(*control.calls.lock().unwrap(), ["move:arm", "move:arm"]);
    }
}
