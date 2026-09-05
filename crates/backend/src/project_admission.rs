//! Project lifetime admission, independent of feature-specific catalog exclusion.
//!
//! An activity retains shared ownership until its last asynchronous consequence
//! completes. A transition can begin only when no admitted activity remains.
//! Project-scoped requests share the lock; replacement owns it exclusively.

use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

use crate::OperationError;

#[derive(Default)]
pub(crate) struct ProjectAdmissionGate {
    ownership: Arc<RwLock<()>>,
}

#[derive(Clone)]
pub(crate) struct ProjectActivity {
    _ownership: Arc<OwnedRwLockReadGuard<()>>,
}

pub(crate) struct ProjectTransition {
    _ownership: OwnedRwLockWriteGuard<()>,
}

impl ProjectAdmissionGate {
    pub(crate) fn admit_activity(&self) -> Result<ProjectActivity, OperationError> {
        self.ownership
            .clone()
            .try_read_owned()
            .map(|ownership| ProjectActivity {
                _ownership: Arc::new(ownership),
            })
            .map_err(|_| OperationError::conflict("Wait for the project change to finish"))
    }

    pub(crate) fn begin_transition(&self) -> Result<ProjectTransition, OperationError> {
        self.ownership
            .clone()
            .try_write_owned()
            .map(|ownership| ProjectTransition {
                _ownership: ownership,
            })
            .map_err(|_| {
                OperationError::conflict(
                    "Stop active work and wait for pending project operations before changing the project or remote worker endpoint",
                )
            })
    }
}

impl ProjectTransition {
    pub(crate) fn into_activity(self) -> ProjectActivity {
        ProjectActivity {
            _ownership: Arc::new(OwnedRwLockWriteGuard::downgrade(self._ownership)),
        }
    }
}
