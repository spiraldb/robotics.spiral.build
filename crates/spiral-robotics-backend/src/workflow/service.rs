//! Delivery-neutral admission policy for saved Workflow graphs and runs.
//!
//! Graph shape and serde belong to the shared API crate. This layer owns
//! revision and single-run concurrency rules without depending on HTTP, Tokio,
//! SpiralDB, or a wire representation.

use spiral_robotics_core::OperationError;

pub(crate) struct WorkflowSaveAdmission {
    expected_revision: u64,
    current_revision: u64,
    run_active: bool,
}

impl WorkflowSaveAdmission {
    pub(crate) const fn new(expected_revision: u64, current_revision: u64) -> Self {
        Self {
            expected_revision,
            current_revision,
            run_active: false,
        }
    }

    pub(crate) const fn with_run_active(mut self, run_active: bool) -> Self {
        self.run_active = run_active;
        self
    }

    pub(crate) fn next_revision(self) -> Result<u64, OperationError> {
        if self.run_active {
            return Err(OperationError::Conflict(
                "Cancel the active Workflow run before applying graph changes.".to_owned(),
            ));
        }
        if self.expected_revision != self.current_revision {
            return Err(OperationError::Conflict(format!(
                "Workflow revision changed from {} to {}; reload before applying edits.",
                self.expected_revision, self.current_revision
            )));
        }
        self.current_revision
            .checked_add(1)
            .ok_or_else(|| OperationError::Internal("Workflow revision overflowed.".to_owned()))
    }
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

pub(crate) struct WorkflowCancellationAdmission {
    run_active: bool,
}

impl WorkflowCancellationAdmission {
    pub(crate) const fn new(run_active: bool) -> Self {
        Self { run_active }
    }

    pub(crate) fn validate(self) -> Result<(), OperationError> {
        if self.run_active {
            Ok(())
        } else {
            Err(OperationError::Conflict(
                "No Workflow run is active.".to_owned(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_requires_matching_revision_and_idle_runtime() {
        assert_eq!(WorkflowSaveAdmission::new(4, 4).next_revision(), Ok(5));
        assert!(matches!(
            WorkflowSaveAdmission::new(3, 4).next_revision(),
            Err(OperationError::Conflict(_))
        ));
        assert!(matches!(
            WorkflowSaveAdmission::new(4, 4)
                .with_run_active(true)
                .next_revision(),
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
        assert_eq!(WorkflowCancellationAdmission::new(true).validate(), Ok(()));
        assert!(matches!(
            WorkflowCancellationAdmission::new(false).validate(),
            Err(OperationError::Conflict(_))
        ));
    }
}
