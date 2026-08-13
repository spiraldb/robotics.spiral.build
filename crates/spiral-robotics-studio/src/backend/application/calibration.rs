//! Calibrate one robot without depending on HTTP, files, or a hardware driver.

use std::collections::BTreeMap;

use spiral_robotics_core::calibration::{
    Calibration, CalibrationInput, MotorSpec, build_calibration,
};

use super::catalog_lease::CatalogLeaseToken;
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

/// Work that would make rewriting completed episode state inconsistent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CalibrationHistoryActivity {
    pub recording: Option<String>,
    pub replaying: Option<String>,
    pub training_active: bool,
    pub publication_active: bool,
}

/// Durable local changes completed before optional SpiralDB synchronization.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReappliedLocalPositions {
    pub updated_episode_ids: Vec<String>,
    pub frames_updated: u64,
    pub episodes_unchanged: u64,
    pub episodes_skipped: u64,
    pub warnings: Vec<String>,
}

/// Result presented to the delivery layer after local state is durable.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReapplyCalibrationOutput {
    pub episodes_updated: u64,
    pub frames_updated: u64,
    pub episodes_unchanged: u64,
    pub episodes_skipped: u64,
    pub spiral_syncs_queued: u64,
    pub warnings: Vec<String>,
}

/// Historical episode boundary for applying a robot's current calibration.
pub(crate) trait CalibrationHistoryPort {
    fn reserve_catalog_mutation(&self) -> Option<CatalogLeaseToken>;
    fn release_catalog_mutation(&self, token: CatalogLeaseToken);
    fn activity(&self) -> CalibrationHistoryActivity;
    fn current_calibration(&self, robot: &str) -> Result<Option<Calibration>, ApplicationError>;

    async fn reapply_local_positions(
        &self,
        robot: String,
        calibration: Calibration,
    ) -> Result<ReappliedLocalPositions, ApplicationError>;

    /// Queue synchronization when a SpiralDB destination is configured.
    /// `false` means the take intentionally remains local.
    fn queue_spiral_sync(&self, episode: String) -> Result<bool, ApplicationError>;
}

struct CalibrationHistoryReservation<'a, P: CalibrationHistoryPort + ?Sized> {
    port: &'a P,
    token: CatalogLeaseToken,
}

impl<P: CalibrationHistoryPort + ?Sized> Drop for CalibrationHistoryReservation<'_, P> {
    fn drop(&mut self) {
        self.port.release_catalog_mutation(self.token);
    }
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

/// Reinterpret every matching completed take from raw encoder counts.
///
/// The current calibration is read once after catalog admission, local files
/// are replaced atomically, and only then are changed takes queued for remote
/// synchronization. Actions remain untouched because old takes do not retain
/// the raw commanded goal needed to transform them faithfully.
pub(crate) async fn reapply_history<P>(
    port: &P,
    robot: &str,
) -> Result<ReapplyCalibrationOutput, ApplicationError>
where
    P: CalibrationHistoryPort + ?Sized,
{
    let Some(token) = port.reserve_catalog_mutation() else {
        return Err(ApplicationError::Conflict(
            "another recording or dataset mutation is active; wait for it to finish".to_owned(),
        ));
    };
    let _reservation = CalibrationHistoryReservation { port, token };
    let activity = port.activity();
    if let Some(episode) = activity.recording {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is still recording; stop it before reapplying calibration"
        )));
    }
    if let Some(episode) = activity.replaying {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is driving an arm; stop replay before reapplying calibration"
        )));
    }
    if activity.training_active {
        return Err(ApplicationError::Conflict(
            "training is reading the episode dataset; stop it before reapplying calibration"
                .to_owned(),
        ));
    }
    if activity.publication_active {
        return Err(ApplicationError::Conflict(
            "an episode is being published; wait for it to finish before reapplying calibration"
                .to_owned(),
        ));
    }

    let calibration = port.current_calibration(robot)?.ok_or_else(|| {
        ApplicationError::InvalidInput(format!(
            "robot {robot:?} must have a saved calibration before positions can be reapplied"
        ))
    })?;
    let local = port
        .reapply_local_positions(robot.to_owned(), calibration)
        .await?;
    let mut output = ReapplyCalibrationOutput {
        episodes_updated: local.updated_episode_ids.len() as u64,
        frames_updated: local.frames_updated,
        episodes_unchanged: local.episodes_unchanged,
        episodes_skipped: local.episodes_skipped,
        warnings: local.warnings,
        ..ReapplyCalibrationOutput::default()
    };
    for episode in local.updated_episode_ids {
        match port.queue_spiral_sync(episode.clone()) {
            Ok(true) => output.spiral_syncs_queued += 1,
            Ok(false) => {}
            Err(error) => output.warnings.push(format!(
                "take {episode:?} was updated locally but could not be queued for SpiralDB: {error}"
            )),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use spiral_robotics_core::calibration::{CalibrationError, Direction, MotorReading};

    use super::*;
    use crate::backend::application::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};

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

    struct History {
        lease: CatalogMutationLease,
        activity: CalibrationHistoryActivity,
        calibration: Option<Calibration>,
        local: ReappliedLocalPositions,
        synced: Mutex<Vec<String>>,
        fail_sync: bool,
    }

    impl CalibrationHistoryPort for History {
        fn reserve_catalog_mutation(&self) -> Option<CatalogLeaseToken> {
            self.lease
                .try_acquire(CatalogLeaseOwner::CalibrationReapply)
        }

        fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
            assert!(self.lease.release(token));
        }

        fn activity(&self) -> CalibrationHistoryActivity {
            self.activity.clone()
        }

        fn current_calibration(
            &self,
            _robot: &str,
        ) -> Result<Option<Calibration>, ApplicationError> {
            Ok(self.calibration.clone())
        }

        async fn reapply_local_positions(
            &self,
            _robot: String,
            _calibration: Calibration,
        ) -> Result<ReappliedLocalPositions, ApplicationError> {
            Ok(self.local.clone())
        }

        fn queue_spiral_sync(&self, episode: String) -> Result<bool, ApplicationError> {
            self.synced.lock().unwrap().push(episode);
            if self.fail_sync {
                Err(ApplicationError::Internal("catalog offline".to_owned()))
            } else {
                Ok(true)
            }
        }
    }

    fn history() -> History {
        let calibration = build_calibration(&motors(), &input()).unwrap();
        History {
            lease: CatalogMutationLease::default(),
            activity: CalibrationHistoryActivity::default(),
            calibration: Some(calibration),
            local: ReappliedLocalPositions {
                updated_episode_ids: vec!["episode_000001".to_owned(), "episode_000002".to_owned()],
                frames_updated: 120,
                episodes_unchanged: 3,
                episodes_skipped: 1,
                warnings: vec!["one old take was incomplete".to_owned()],
            },
            synced: Mutex::new(Vec::new()),
            fail_sync: false,
        }
    }

    #[tokio::test]
    async fn historical_state_is_rewritten_before_changed_takes_are_synced() {
        let history = history();
        let output = reapply_history(&history, "arm").await.unwrap();

        assert_eq!(output.episodes_updated, 2);
        assert_eq!(output.frames_updated, 120);
        assert_eq!(output.episodes_unchanged, 3);
        assert_eq!(output.episodes_skipped, 1);
        assert_eq!(output.spiral_syncs_queued, 2);
        assert_eq!(
            *history.synced.lock().unwrap(),
            ["episode_000001", "episode_000002"]
        );
        assert!(!history.lease.is_held());
    }

    #[tokio::test]
    async fn remote_sync_failure_does_not_undo_durable_local_recalibration() {
        let mut history = history();
        history.fail_sync = true;

        let output = reapply_history(&history, "arm").await.unwrap();

        assert_eq!(output.episodes_updated, 2);
        assert_eq!(output.spiral_syncs_queued, 0);
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| warning.contains("offline"))
        );
    }

    #[tokio::test]
    async fn active_episode_consumers_block_historical_recalibration() {
        for activity in [
            CalibrationHistoryActivity {
                recording: Some("episode_000003".to_owned()),
                ..CalibrationHistoryActivity::default()
            },
            CalibrationHistoryActivity {
                replaying: Some("episode_000003".to_owned()),
                ..CalibrationHistoryActivity::default()
            },
            CalibrationHistoryActivity {
                training_active: true,
                ..CalibrationHistoryActivity::default()
            },
            CalibrationHistoryActivity {
                publication_active: true,
                ..CalibrationHistoryActivity::default()
            },
        ] {
            let mut history = history();
            history.activity = activity;
            let error = reapply_history(&history, "arm").await.unwrap_err();
            assert!(matches!(error, ApplicationError::Conflict(_)));
            assert!(history.synced.lock().unwrap().is_empty());
            assert!(!history.lease.is_held());
        }
    }
}
