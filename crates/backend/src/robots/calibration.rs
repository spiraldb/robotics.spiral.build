//! Calibrate one robot without depending on HTTP, files, or a hardware driver.

use std::collections::BTreeMap;

use spiral_robotics_common::robots::config::MotorRange;

use crate::OperationError;

/// Saved robot configuration required by the calibration action.
pub(crate) trait RobotCalibrationStore {
    /// Record a sweep's ranges, which must cover exactly the saved motors.
    async fn save_calibration(
        &self,
        robot: &str,
        ranges: BTreeMap<String, MotorRange>,
    ) -> Result<(), OperationError>;
}

/// Live sweep operations required by the calibration action.
pub(crate) trait CalibrationControlPort {
    async fn start_calibration(&self, robot: &str) -> Result<(), OperationError>;

    async fn calibration_ranges(
        &self,
        robot: &str,
    ) -> Result<BTreeMap<String, MotorRange>, OperationError>;

    async fn cancel_calibration(&self, robot: &str) -> Result<(), OperationError>;

    async fn apply_calibration(
        &self,
        robot: &str,
        ranges: BTreeMap<String, MotorRange>,
    ) -> Result<(), OperationError>;
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
    pub(super) const fn new(store: &'a R, control: &'a C) -> Self {
        Self { store, control }
    }

    /// Finish a sweep, persist its ranges, then refresh live control.
    ///
    /// The ordering is part of the product contract: a read or write failure
    /// keeps the sweep alive so the operator does not lose collected motion.
    /// Once persistence succeeds, later adapter failures come back as a warning
    /// because the next connection loads the already-correct saved robot.
    pub(super) async fn finish(&self, robot: &str) -> Result<Option<String>, OperationError> {
        let ranges = self.control.calibration_ranges(robot).await?;
        self.store.save_calibration(robot, ranges.clone()).await?;

        let cancel_warning = self
            .control
            .cancel_calibration(robot)
            .await
            .err()
            .map(|error| format!("could not close the completed sweep: {error}"));
        let apply_warning = self
            .control
            .apply_calibration(robot, ranges)
            .await
            .err()
            .map(|error| format!("could not refresh the live robot: {error}"));
        Ok(match (cancel_warning, apply_warning) {
            (None, None) => None,
            (Some(warning), None) | (None, Some(warning)) => Some(warning),
            (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct Store {
        events: RefCell<Vec<&'static str>>,
        fail_save: bool,
    }

    impl RobotCalibrationStore for Store {
        async fn save_calibration(
            &self,
            _robot: &str,
            _ranges: BTreeMap<String, MotorRange>,
        ) -> Result<(), OperationError> {
            self.events.borrow_mut().push("save");
            if self.fail_save {
                Err(OperationError::Internal("storage is busy".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    struct Control {
        events: RefCell<Vec<&'static str>>,
        fail_cancel: bool,
        fail_apply: bool,
    }

    impl CalibrationControlPort for Control {
        async fn start_calibration(&self, _robot: &str) -> Result<(), OperationError> {
            self.events.borrow_mut().push("start");
            Ok(())
        }

        async fn calibration_ranges(
            &self,
            _robot: &str,
        ) -> Result<BTreeMap<String, MotorRange>, OperationError> {
            self.events.borrow_mut().push("ranges");
            Ok(BTreeMap::from([(
                "shoulder".to_owned(),
                MotorRange::new(100, 500, 900).unwrap(),
            )]))
        }

        async fn cancel_calibration(&self, _robot: &str) -> Result<(), OperationError> {
            self.events.borrow_mut().push("cancel");
            if self.fail_cancel {
                Err(OperationError::Conflict("control stopped".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn apply_calibration(
            &self,
            _robot: &str,
            _ranges: BTreeMap<String, MotorRange>,
        ) -> Result<(), OperationError> {
            self.events.borrow_mut().push("apply");
            if self.fail_apply {
                Err(OperationError::Conflict("robot left".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    fn setup(fail_save: bool) -> (Store, Control) {
        (
            Store {
                events: RefCell::default(),
                fail_save,
            },
            Control {
                events: RefCell::default(),
                fail_cancel: false,
                fail_apply: false,
            },
        )
    }

    #[tokio::test]
    async fn persistence_failure_keeps_the_sweep_alive() {
        let (store, control) = setup(true);
        let error = CalibrationActions::new(&store, &control)
            .finish("arm")
            .await
            .unwrap_err();
        assert_eq!(
            error,
            OperationError::Internal("storage is busy".to_owned())
        );
        assert_eq!(*store.events.borrow(), ["save"]);
        assert_eq!(*control.events.borrow(), ["ranges"]);
    }

    #[tokio::test]
    async fn a_saved_calibration_survives_live_refresh_failures() {
        let (store, mut control) = setup(false);
        control.fail_cancel = true;
        control.fail_apply = true;
        let warning = CalibrationActions::new(&store, &control)
            .finish("arm")
            .await
            .unwrap();
        assert!(warning.is_some_and(|warning| {
            warning.contains("control stopped") && warning.contains("robot left")
        }));
        assert_eq!(*store.events.borrow(), ["save"]);
        assert_eq!(*control.events.borrow(), ["ranges", "cancel", "apply"]);
    }
}
