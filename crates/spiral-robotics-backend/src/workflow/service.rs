//! Delivery-neutral admission policy for saved Workflow graphs and runs.
//!
//! Graph shape and serde belong to the shared API crate. This layer owns
//! revision and single-run concurrency rules without depending on HTTP, Tokio,
//! SpiralDB, or a wire representation.

use spiral_robotics_core::OperationError;

pub(crate) fn admit_save(
    expected_revision: u64,
    current_revision: u64,
    run_active: bool,
) -> Result<u64, OperationError> {
    if run_active {
        return Err(OperationError::Conflict(
            "Cancel the active Workflow run before applying graph changes.".to_owned(),
        ));
    }
    if expected_revision != current_revision {
        return Err(OperationError::Conflict(format!(
            "Workflow revision changed from {expected_revision} to {current_revision}; reload before applying edits."
        )));
    }
    current_revision
        .checked_add(1)
        .ok_or_else(|| OperationError::Internal("Workflow revision overflowed.".to_owned()))
}

pub(crate) struct WorkflowRunAdmission {
    requested_revision: u64,
    current_revision: u64,
    run_active: bool,
    valid: bool,
}

impl WorkflowRunAdmission {
    pub(crate) fn new(requested_revision: u64, current_revision: u64) -> Self {
        Self {
            requested_revision,
            current_revision,
            run_active: false,
            valid: false,
        }
    }

    pub(crate) fn with_state(mut self, run_active: bool, valid: bool) -> Self {
        self.run_active = run_active;
        self.valid = valid;
        self
    }

    pub(crate) fn validate(self) -> Result<(), OperationError> {
        if self.run_active {
            return Err(OperationError::Conflict(
                "This Workflow already has an active run.".to_owned(),
            ));
        }
        if self.requested_revision != self.current_revision {
            return Err(OperationError::Conflict(format!(
                "Workflow revision {} is stale; current revision is {}.",
                self.requested_revision, self.current_revision
            )));
        }
        if !self.valid {
            return Err(OperationError::InvalidInput(
                "Apply a valid Table → Query → Result graph before running.".to_owned(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn admit_cancel(run_active: bool) -> Result<(), OperationError> {
    if run_active {
        Ok(())
    } else {
        Err(OperationError::Conflict(
            "No Workflow run is active.".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_requires_matching_revision_and_idle_runtime() {
        assert_eq!(admit_save(4, 4, false), Ok(5));
        assert!(matches!(
            admit_save(3, 4, false),
            Err(OperationError::Conflict(_))
        ));
        assert!(matches!(
            admit_save(4, 4, true),
            Err(OperationError::Conflict(_))
        ));
    }

    #[test]
    fn run_rejects_concurrency_stale_and_invalid_graphs() {
        assert_eq!(
            WorkflowRunAdmission::new(2, 2)
                .with_state(false, true)
                .validate(),
            Ok(())
        );
        for result in [
            WorkflowRunAdmission::new(2, 2)
                .with_state(true, true)
                .validate(),
            WorkflowRunAdmission::new(1, 2)
                .with_state(false, true)
                .validate(),
            WorkflowRunAdmission::new(2, 2)
                .with_state(false, false)
                .validate(),
        ] {
            assert!(result.is_err());
        }
    }

    #[test]
    fn cancel_requires_an_active_run() {
        assert_eq!(admit_cancel(true), Ok(()));
        assert!(matches!(
            admit_cancel(false),
            Err(OperationError::Conflict(_))
        ));
    }
}
