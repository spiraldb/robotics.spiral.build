//! Operator-requested control-mode changes.

use crate::OperationError;

/// Modes an operator may enter directly.
///
/// Policy sessions are deliberately absent: their use case must load and
/// validate a checkpoint before control can change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperatorMode {
    Idle,
    Manual { robot: String },
    Teleoperate { leader: String, follower: String },
}

pub(crate) trait ModeControlPort {
    async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), OperationError>;
}

pub(crate) struct ModeActions<'a, P: ModeControlPort + ?Sized> {
    control: &'a P,
}

impl<'a, P: ModeControlPort + ?Sized> ModeActions<'a, P> {
    pub(super) const fn new(control: &'a P) -> Self {
        Self { control }
    }

    pub(super) async fn set(&self, mode: OperatorMode) -> Result<(), OperationError> {
        match &mode {
            OperatorMode::Idle => {}
            OperatorMode::Manual { robot } if robot.trim().is_empty() => {
                return Err(OperationError::InvalidInput(
                    "manual control requires a robot".to_owned(),
                ));
            }
            OperatorMode::Manual { .. } => {}
            OperatorMode::Teleoperate { leader, follower }
                if leader.trim().is_empty() || follower.trim().is_empty() =>
            {
                return Err(OperationError::InvalidInput(
                    "teleoperation requires a leader and follower".to_owned(),
                ));
            }
            OperatorMode::Teleoperate { leader, follower } if leader == follower => {
                return Err(OperationError::InvalidInput(
                    "teleoperation leader and follower must be different robots".to_owned(),
                ));
            }
            OperatorMode::Teleoperate { .. } => {}
        }
        self.control.set_operator_mode(mode).await
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct FakeControl(RefCell<Vec<OperatorMode>>);

    impl ModeControlPort for FakeControl {
        async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), OperationError> {
            self.0.borrow_mut().push(mode);
            Ok(())
        }
    }

    #[tokio::test]
    async fn one_capability_action_enters_teleoperation() {
        let control = FakeControl::default();
        let mode = OperatorMode::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        };
        ModeActions::new(&control).set(mode.clone()).await.unwrap();
        assert_eq!(*control.0.borrow_mut(), [mode]);
    }

    #[tokio::test]
    async fn rejects_self_teleoperation_before_reaching_the_adapter() {
        let control = FakeControl::default();
        let error = ModeActions::new(&control)
            .set(OperatorMode::Teleoperate {
                leader: "arm".to_owned(),
                follower: "arm".to_owned(),
            })
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::InvalidInput(_)));
        assert!(control.0.borrow_mut().is_empty());
    }
}
