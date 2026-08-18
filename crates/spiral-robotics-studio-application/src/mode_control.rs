//! Operator-requested control-mode changes.

use super::error::ApplicationError;

/// Modes an operator may enter directly.
///
/// Replay and policy sessions are deliberately absent: their use cases must
/// load and validate an episode or checkpoint before control can change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorMode {
    Idle,
    Manual { robot: String },
    Teleoperate { leader: String, follower: String },
}

pub trait ModeControlPort {
    async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), ApplicationError>;
}

pub async fn set_mode<P: ModeControlPort + ?Sized>(
    control: &P,
    mode: OperatorMode,
) -> Result<(), ApplicationError> {
    match &mode {
        OperatorMode::Idle => {}
        OperatorMode::Manual { robot } if robot.trim().is_empty() => {
            return Err(ApplicationError::InvalidInput(
                "manual control requires a robot".to_owned(),
            ));
        }
        OperatorMode::Manual { .. } => {}
        OperatorMode::Teleoperate { leader, follower }
            if leader.trim().is_empty() || follower.trim().is_empty() =>
        {
            return Err(ApplicationError::InvalidInput(
                "teleoperation requires a leader and follower".to_owned(),
            ));
        }
        OperatorMode::Teleoperate { leader, follower } if leader == follower => {
            return Err(ApplicationError::InvalidInput(
                "teleoperation leader and follower must be different robots".to_owned(),
            ));
        }
        OperatorMode::Teleoperate { .. } => {}
    }
    control.set_operator_mode(mode).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeControl(Mutex<Vec<OperatorMode>>);

    impl ModeControlPort for FakeControl {
        async fn set_operator_mode(&self, mode: OperatorMode) -> Result<(), ApplicationError> {
            self.0.lock().unwrap().push(mode);
            Ok(())
        }
    }

    #[tokio::test]
    async fn one_application_action_enters_teleoperation() {
        let control = FakeControl::default();
        let mode = OperatorMode::Teleoperate {
            leader: "leader".to_owned(),
            follower: "follower".to_owned(),
        };
        set_mode(&control, mode.clone()).await.unwrap();
        assert_eq!(*control.0.lock().unwrap(), [mode]);
    }

    #[tokio::test]
    async fn rejects_self_teleoperation_before_reaching_the_adapter() {
        let control = FakeControl::default();
        let error = set_mode(
            &control,
            OperatorMode::Teleoperate {
                leader: "arm".to_owned(),
                follower: "arm".to_owned(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ApplicationError::InvalidInput(_)));
        assert!(control.0.lock().unwrap().is_empty());
    }
}
