//! Calibrate one robot without depending on HTTP, files, or a hardware driver.

use std::collections::BTreeMap;

use spiral_robotics_hardware::{Calibration, CalibrationInput, MotorSpec};

use crate::OperationError;

pub(crate) struct CalibrationUpdate<'a> {
    motors: &'a BTreeMap<String, MotorSpec>,
    calibration: &'a Calibration,
}

impl<'a> CalibrationUpdate<'a> {
    pub(crate) fn new(
        motors: &'a BTreeMap<String, MotorSpec>,
        calibration: &'a Calibration,
    ) -> Self {
        Self {
            motors,
            calibration,
        }
    }

    pub(crate) fn motors(&self) -> &BTreeMap<String, MotorSpec> {
        self.motors
    }

    pub(crate) fn calibration(&self) -> &Calibration {
        self.calibration
    }
}

/// Saved robot configuration required by the calibration action.
pub(crate) trait RobotCalibrationStore {
    async fn motor_specs(&self, robot: &str)
    -> Result<BTreeMap<String, MotorSpec>, OperationError>;

    async fn save_calibration(
        &self,
        robot: &str,
        update: CalibrationUpdate<'_>,
    ) -> Result<(), OperationError>;
}

/// Live sweep operations required by the calibration action.
pub(crate) trait CalibrationControlPort {
    async fn start_calibration(&self, robot: &str) -> Result<(), OperationError>;

    async fn calibration_input(&self, robot: &str) -> Result<CalibrationInput, OperationError>;

    async fn cancel_calibration(&self, robot: &str) -> Result<(), OperationError>;

    async fn apply_calibration(
        &self,
        robot: &str,
        calibration: Calibration,
    ) -> Result<(), OperationError>;
}

pub(crate) struct CalibrationControlActions<'a, C: CalibrationControlPort + ?Sized> {
    control: &'a C,
}

impl<'a, C: CalibrationControlPort + ?Sized> CalibrationControlActions<'a, C> {
    pub(crate) const fn new(control: &'a C) -> Self {
        Self { control }
    }

    /// Begin a hand-guided calibration sweep.
    pub(crate) async fn start(&self, robot: &str) -> Result<(), OperationError> {
        self.control.start_calibration(robot).await
    }

    /// Abandon a calibration sweep and return the robot to normal operation.
    pub(crate) async fn cancel(&self, robot: &str) -> Result<(), OperationError> {
        self.control.cancel_calibration(robot).await
    }
}

/// A calibration was durably saved. Any warning concerns only its live refresh.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FinishCalibrationOutput {
    calibration: Calibration,
    live_warning: Option<String>,
}

impl FinishCalibrationOutput {
    pub(crate) fn live_warning(&self) -> Option<&str> {
        self.live_warning.as_deref()
    }
}

pub(crate) struct CalibrationActions<
    'a,
    R: RobotCalibrationStore + ?Sized,
    C: CalibrationControlPort + ?Sized,
> {
    store: &'a R,
    control: &'a C,
}

impl<'a, R, C> CalibrationActions<'a, R, C>
where
    R: RobotCalibrationStore + ?Sized,
    C: CalibrationControlPort + ?Sized,
{
    pub(crate) const fn new(store: &'a R, control: &'a C) -> Self {
        Self { store, control }
    }

    /// Finish a sweep, validate its domain result, persist it, then refresh live control.
    ///
    /// The ordering is part of the product contract: a read/build/write failure
    /// keeps the sweep alive so the operator does not lose collected motion. Once
    /// persistence succeeds, later adapter failures become warnings because the
    /// next connection can load the already-correct SpiralDB row.
    pub(crate) async fn finish(
        &self,
        robot: &str,
    ) -> Result<FinishCalibrationOutput, OperationError> {
        let motors = self.store.motor_specs(robot).await?;
        let input = self.control.calibration_input(robot).await?;
        let calibration = input
            .build(&motors)
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        self.store
            .save_calibration(robot, CalibrationUpdate::new(&motors, &calibration))
            .await?;

        let cancel_warning = self
            .control
            .cancel_calibration(robot)
            .await
            .err()
            .map(|error| format!("could not close the completed sweep: {error}"));
        let apply_warning = self
            .control
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
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use spiral_robotics_hardware::{CalibrationError, MotorDirection, MotorReading};

    use super::*;

    fn motors() -> BTreeMap<String, MotorSpec> {
        BTreeMap::from([(
            "shoulder".to_owned(),
            MotorSpec::new(1, MotorDirection::Normal),
        )])
    }

    fn input() -> CalibrationInput {
        CalibrationInput::Direct {
            motors: BTreeMap::from([("shoulder".to_owned(), MotorReading::new(500, 100, 900))]),
        }
    }

    struct Store {
        events: Mutex<Vec<&'static str>>,
        fail_save: bool,
    }

    impl RobotCalibrationStore for Store {
        async fn motor_specs(
            &self,
            _robot: &str,
        ) -> Result<BTreeMap<String, MotorSpec>, OperationError> {
            self.events.lock().unwrap().push("specs");
            Ok(motors())
        }

        async fn save_calibration(
            &self,
            _robot: &str,
            _update: CalibrationUpdate<'_>,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("save");
            if self.fail_save {
                Err(OperationError::Internal("Spiral is busy".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    struct Control {
        events: Mutex<Vec<&'static str>>,
        input: Result<CalibrationInput, OperationError>,
        fail_start: bool,
        fail_cancel: bool,
        fail_apply: bool,
    }

    impl CalibrationControlPort for Control {
        async fn start_calibration(&self, _robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("start");
            if self.fail_start {
                Err(OperationError::Conflict("robot is busy".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn calibration_input(
            &self,
            _robot: &str,
        ) -> Result<CalibrationInput, OperationError> {
            self.events.lock().unwrap().push("input");
            self.input.clone()
        }

        async fn cancel_calibration(&self, _robot: &str) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("cancel");
            if self.fail_cancel {
                Err(OperationError::Conflict("control stopped".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn apply_calibration(
            &self,
            _robot: &str,
            _calibration: Calibration,
        ) -> Result<(), OperationError> {
            self.events.lock().unwrap().push("apply");
            if self.fail_apply {
                Err(OperationError::Conflict("robot left".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    fn setup(fail_save: bool, input: Result<CalibrationInput, OperationError>) -> (Store, Control) {
        (
            Store {
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
        let actions = CalibrationControlActions::new(&control);
        actions.start("arm").await.unwrap();
        actions.cancel("arm").await.unwrap();
        assert_eq!(*control.events.lock().unwrap(), ["start", "cancel"]);
    }

    #[tokio::test]
    async fn begin_refusal_propagates_without_compensation() {
        let (_repository, mut control) = setup(false, Ok(input()));
        control.fail_start = true;
        let error = CalibrationControlActions::new(&control)
            .start("arm")
            .await
            .unwrap_err();
        assert_eq!(error, OperationError::Conflict("robot is busy".to_owned()));
        assert_eq!(*control.events.lock().unwrap(), ["start"]);
    }

    #[tokio::test]
    async fn persistence_failure_keeps_the_sweep_alive() {
        let (store, control) = setup(true, Ok(input()));
        let error = CalibrationActions::new(&store, &control)
            .finish("arm")
            .await
            .unwrap_err();
        assert_eq!(error, OperationError::Internal("Spiral is busy".to_owned()));
        assert_eq!(*store.events.lock().unwrap(), ["specs", "save"]);
        assert_eq!(*control.events.lock().unwrap(), ["input"]);
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_storage_or_cleanup() {
        let mut incomplete = input();
        let CalibrationInput::Direct { motors } = &mut incomplete else {
            unreachable!()
        };
        motors.clear();
        let (store, control) = setup(false, Ok(incomplete));
        let error = CalibrationActions::new(&store, &control)
            .finish("arm")
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::InvalidInput(_)));
        assert_eq!(*store.events.lock().unwrap(), ["specs"]);
        assert_eq!(*control.events.lock().unwrap(), ["input"]);
    }

    #[tokio::test]
    async fn a_saved_calibration_survives_live_refresh_failures() {
        let (store, mut control) = setup(false, Ok(input()));
        control.fail_cancel = true;
        control.fail_apply = true;
        let output = CalibrationActions::new(&store, &control)
            .finish("arm")
            .await
            .unwrap();
        assert!(output.live_warning.as_deref().is_some_and(|warning| {
            warning.contains("control stopped") && warning.contains("robot left")
        }));
        assert_eq!(*store.events.lock().unwrap(), ["specs", "save"]);
        assert_eq!(
            *control.events.lock().unwrap(),
            ["input", "cancel", "apply"]
        );
    }

    #[test]
    fn calibration_error_remains_a_domain_error_until_the_use_case_boundary() {
        let error = CalibrationInput::Direct {
            motors: BTreeMap::new(),
        }
        .build(&motors())
        .unwrap_err();
        assert!(matches!(error, CalibrationError::KeyMismatch { .. }));
    }
}
