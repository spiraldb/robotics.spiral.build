//! Calibrate one robot without depending on HTTP, files, or a hardware driver.

use std::collections::BTreeMap;

use spiral_robotics_core::calibration::{
    Calibration, CalibrationInput, MotorSpec, build_calibration,
};

use super::error::ApplicationError;

/// Saved robot configuration required by the calibration action.
pub(crate) trait RobotCalibrationRepository {
    fn motor_specs(&self, robot: &str) -> Result<BTreeMap<String, MotorSpec>, ApplicationError>;

    fn save_calibration(
        &self,
        robot: &str,
        motors: &BTreeMap<String, MotorSpec>,
        calibration: &Calibration,
    ) -> Result<(), ApplicationError>;
}

/// Live sweep operations required by the calibration action.
pub(crate) trait CalibrationControlPort {
    async fn start_calibration(&self, robot: &str) -> Result<(), ApplicationError>;

    async fn calibration_input(&self, robot: &str) -> Result<CalibrationInput, ApplicationError>;

    async fn cancel_calibration(&self, robot: &str) -> Result<(), ApplicationError>;

    async fn apply_calibration(
        &self,
        robot: &str,
        calibration: Calibration,
    ) -> Result<(), ApplicationError>;
}

/// Begin a hand-guided calibration sweep.
pub(crate) async fn start<C>(control: &C, robot: &str) -> Result<(), ApplicationError>
where
    C: CalibrationControlPort + ?Sized,
{
    control.start_calibration(robot).await
}

/// Abandon a calibration sweep and return the robot to normal operation.
pub(crate) async fn cancel<C>(control: &C, robot: &str) -> Result<(), ApplicationError>
where
    C: CalibrationControlPort + ?Sized,
{
    control.cancel_calibration(robot).await
}

/// A calibration was durably saved. Any warning concerns only its live refresh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FinishCalibrationOutput {
    pub calibration: Calibration,
    pub live_warning: Option<String>,
}

/// Finish a sweep, validate its domain result, persist it, then refresh live control.
///
/// The ordering is part of the product contract: a read/build/write failure
/// keeps the sweep alive so the operator does not lose collected motion. Once
/// persistence succeeds, later adapter failures become warnings because the
/// next connection can load the already-correct file.
pub(crate) async fn finish<R, C>(
    repository: &R,
    control: &C,
    robot: &str,
) -> Result<FinishCalibrationOutput, ApplicationError>
where
    R: RobotCalibrationRepository + ?Sized,
    C: CalibrationControlPort + ?Sized,
{
    let motors = repository.motor_specs(robot)?;
    let input = control.calibration_input(robot).await?;
    let calibration = build_calibration(&motors, &input)
        .map_err(|error| ApplicationError::InvalidInput(error.to_string()))?;
    repository.save_calibration(robot, &motors, &calibration)?;

    let cancel_warning = control
        .cancel_calibration(robot)
        .await
        .err()
        .map(|error| format!("could not close the completed sweep: {error}"));
    let apply_warning = control
        .apply_calibration(robot, calibration.clone())
        .await
        .err()
        .map(|error| format!("could not refresh the live robot: {error}"));
    let live_warning = match (cancel_warning, apply_warning) {
        (None, None) => None,
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
    };

    Ok(FinishCalibrationOutput {
        calibration,
        live_warning,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use spiral_robotics_core::calibration::{CalibrationError, Direction, MotorReading};

    use super::*;

    fn motors() -> BTreeMap<String, MotorSpec> {
        BTreeMap::from([(
            "shoulder".to_owned(),
            MotorSpec {
                id: 1,
                direction: Direction::Normal,
                model: None,
            },
        )])
    }

    fn input() -> CalibrationInput {
        CalibrationInput::Direct {
            motors: BTreeMap::from([(
                "shoulder".to_owned(),
                MotorReading {
                    center: 500,
                    range_min: 100,
                    range_max: 900,
                },
            )]),
        }
    }

    struct Repository {
        events: Mutex<Vec<&'static str>>,
        fail_save: bool,
    }

    impl RobotCalibrationRepository for Repository {
        fn motor_specs(
            &self,
            _robot: &str,
        ) -> Result<BTreeMap<String, MotorSpec>, ApplicationError> {
            self.events.lock().unwrap().push("specs");
            Ok(motors())
        }

        fn save_calibration(
            &self,
            _robot: &str,
            _motors: &BTreeMap<String, MotorSpec>,
            _calibration: &Calibration,
        ) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push("save");
            if self.fail_save {
                Err(ApplicationError::Internal("disk is busy".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    struct Control {
        events: Mutex<Vec<&'static str>>,
        input: Result<CalibrationInput, ApplicationError>,
        fail_start: bool,
        fail_cancel: bool,
        fail_apply: bool,
    }

    impl CalibrationControlPort for Control {
        async fn start_calibration(&self, _robot: &str) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push("start");
            if self.fail_start {
                Err(ApplicationError::Conflict("robot is busy".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn calibration_input(
            &self,
            _robot: &str,
        ) -> Result<CalibrationInput, ApplicationError> {
            self.events.lock().unwrap().push("input");
            self.input.clone()
        }

        async fn cancel_calibration(&self, _robot: &str) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push("cancel");
            if self.fail_cancel {
                Err(ApplicationError::Conflict("control stopped".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn apply_calibration(
            &self,
            _robot: &str,
            _calibration: Calibration,
        ) -> Result<(), ApplicationError> {
            self.events.lock().unwrap().push("apply");
            if self.fail_apply {
                Err(ApplicationError::Conflict("robot left".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    fn setup(
        fail_save: bool,
        input: Result<CalibrationInput, ApplicationError>,
    ) -> (Repository, Control) {
        (
            Repository {
                events: Mutex::new(Vec::new()),
                fail_save,
            },
            Control {
                events: Mutex::new(Vec::new()),
                input,
                fail_start: false,
                fail_cancel: false,
                fail_apply: false,
            },
        )
    }

    #[tokio::test]
    async fn begin_and_cancel_are_transport_neutral_actions() {
        let (_repository, control) = setup(false, Ok(input()));
        start(&control, "arm").await.unwrap();
        cancel(&control, "arm").await.unwrap();
        assert_eq!(*control.events.lock().unwrap(), ["start", "cancel"]);
    }

    #[tokio::test]
    async fn begin_refusal_propagates_without_compensation() {
        let (_repository, mut control) = setup(false, Ok(input()));
        control.fail_start = true;
        let error = start(&control, "arm").await.unwrap_err();
        assert_eq!(
            error,
            ApplicationError::Conflict("robot is busy".to_owned())
        );
        assert_eq!(*control.events.lock().unwrap(), ["start"]);
    }

    #[tokio::test]
    async fn persistence_failure_keeps_the_sweep_alive() {
        let (repository, control) = setup(true, Ok(input()));
        let error = finish(&repository, &control, "arm").await.unwrap_err();
        assert_eq!(error, ApplicationError::Internal("disk is busy".to_owned()));
        assert_eq!(*repository.events.lock().unwrap(), ["specs", "save"]);
        assert_eq!(*control.events.lock().unwrap(), ["input"]);
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_storage_or_cleanup() {
        let mut incomplete = input();
        let CalibrationInput::Direct { motors } = &mut incomplete else {
            unreachable!()
        };
        motors.clear();
        let (repository, control) = setup(false, Ok(incomplete));
        let error = finish(&repository, &control, "arm").await.unwrap_err();
        assert!(matches!(error, ApplicationError::InvalidInput(_)));
        assert_eq!(*repository.events.lock().unwrap(), ["specs"]);
        assert_eq!(*control.events.lock().unwrap(), ["input"]);
    }

    #[tokio::test]
    async fn a_saved_calibration_survives_live_refresh_failures() {
        let (repository, mut control) = setup(false, Ok(input()));
        control.fail_cancel = true;
        control.fail_apply = true;
        let output = finish(&repository, &control, "arm").await.unwrap();
        assert!(output.live_warning.as_deref().is_some_and(|warning| {
            warning.contains("control stopped") && warning.contains("robot left")
        }));
        assert_eq!(*repository.events.lock().unwrap(), ["specs", "save"]);
        assert_eq!(
            *control.events.lock().unwrap(),
            ["input", "cancel", "apply"]
        );
    }

    #[test]
    fn calibration_error_remains_a_domain_error_until_the_use_case_boundary() {
        let error = build_calibration(
            &motors(),
            &CalibrationInput::Direct {
                motors: BTreeMap::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, CalibrationError::KeyMismatch { .. }));
    }
}
