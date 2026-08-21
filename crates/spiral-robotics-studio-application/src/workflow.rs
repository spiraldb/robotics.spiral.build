//! Delivery-neutral admission policy for saved Workflow graphs and runs.
//!
//! Graph shape and serde belong to the shared contracts crate. This layer owns
//! revision and single-run concurrency rules without depending on HTTP, Tokio,
//! SpiralDB, or a wire representation.

use crate::error::ApplicationError;

pub const MODEL_STAGE_THREE_RECOVERY: &str =
    "Remove Model nodes to run SQL now; Model execution arrives in Stage 3.";

pub fn admit_save(
    expected_revision: u64,
    current_revision: u64,
    run_active: bool,
) -> Result<u64, ApplicationError> {
    if run_active {
        return Err(ApplicationError::Conflict(
            "Cancel the active Workflow run before applying graph changes.".to_owned(),
        ));
    }
    if expected_revision != current_revision {
        return Err(ApplicationError::Conflict(format!(
            "Workflow revision changed from {expected_revision} to {current_revision}; reload before applying edits."
        )));
    }
    current_revision
        .checked_add(1)
        .ok_or_else(|| ApplicationError::Internal("Workflow revision overflowed.".to_owned()))
}

pub fn admit_run(
    requested_revision: u64,
    current_revision: u64,
    run_active: bool,
    valid: bool,
    contains_model: bool,
) -> Result<(), ApplicationError> {
    if run_active {
        return Err(ApplicationError::Conflict(
            "This Workflow already has an active run.".to_owned(),
        ));
    }
    if requested_revision != current_revision {
        return Err(ApplicationError::Conflict(format!(
            "Workflow revision {requested_revision} is stale; current revision is {current_revision}."
        )));
    }
    if !valid {
        return Err(ApplicationError::InvalidInput(
            "Apply a valid Table → Query → Result graph before running.".to_owned(),
        ));
    }
    if contains_model {
        return Err(ApplicationError::InvalidInput(
            MODEL_STAGE_THREE_RECOVERY.to_owned(),
        ));
    }
    Ok(())
}

pub fn admit_cancel(run_active: bool) -> Result<(), ApplicationError> {
    if run_active {
        Ok(())
    } else {
        Err(ApplicationError::Conflict(
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
            Err(ApplicationError::Conflict(_))
        ));
        assert!(matches!(
            admit_save(4, 4, true),
            Err(ApplicationError::Conflict(_))
        ));
    }

    #[test]
    fn run_rejects_concurrency_stale_invalid_and_model_graphs() {
        assert_eq!(admit_run(2, 2, false, true, false), Ok(()));
        for result in [
            admit_run(2, 2, true, true, false),
            admit_run(1, 2, false, true, false),
            admit_run(2, 2, false, false, false),
            admit_run(2, 2, false, true, true),
        ] {
            assert!(result.is_err());
        }
        assert_eq!(
            admit_run(2, 2, false, true, true).unwrap_err().message(),
            MODEL_STAGE_THREE_RECOVERY
        );
    }

    #[test]
    fn cancel_requires_an_active_run() {
        assert_eq!(admit_cancel(true), Ok(()));
        assert!(matches!(
            admit_cancel(false),
            Err(ApplicationError::Conflict(_))
        ));
    }
}
