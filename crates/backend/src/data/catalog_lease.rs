//! Exclusive, owner-scoped admission for episode-catalog mutation.
//!
//! Recording and dataset import can mutate the same catalog.
//! A boolean cannot express who owns that exclusion, so an unrelated stop can
//! accidentally release somebody else's mutation. This lease uses an opaque
//! generation token: only the exact acquisition may release its slot.

use std::sync::{Arc, Mutex};

use crate::sync::MutexRecovery;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogLeaseOwner {
    Recording,
    DatasetImport,
    WorkflowStart,
    CurationStart,
    StudioCatalogRead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogLeaseToken {
    owner: CatalogLeaseOwner,
    generation: u64,
}

struct LeaseHolder {
    token: CatalogLeaseToken,
}

#[derive(Default)]
struct LeaseState {
    generation: u64,
    holder: Option<LeaseHolder>,
}

#[derive(Default)]
pub(crate) struct CatalogMutationLease {
    state: Mutex<LeaseState>,
}

/// Releases one short-lived catalog acquisition when its command scope ends.
/// Recording keeps the token directly because its lease spans several state
/// transitions; ordinary catalog operations should use this guard.
pub(crate) struct CatalogLeaseReservation {
    lease: Arc<CatalogMutationLease>,
    token: CatalogLeaseToken,
}

impl Drop for CatalogLeaseReservation {
    fn drop(&mut self) {
        let released = self.lease.release(self.token);
        debug_assert!(released, "catalog operation releases its exact lease once");
    }
}

impl CatalogMutationLease {
    pub(crate) fn try_reserve(
        self: &Arc<Self>,
        owner: CatalogLeaseOwner,
    ) -> Option<CatalogLeaseReservation> {
        self.try_acquire(owner)
            .map(|token| CatalogLeaseReservation {
                lease: Arc::clone(self),
                token,
            })
    }

    pub(crate) fn try_acquire(&self, owner: CatalogLeaseOwner) -> Option<CatalogLeaseToken> {
        let mut state = self.state.recover_lock();
        if state.holder.is_some() {
            return None;
        }
        state.generation = state.generation.wrapping_add(1).max(1);
        let token = CatalogLeaseToken {
            owner,
            generation: state.generation,
        };
        state.holder = Some(LeaseHolder { token });
        Some(token)
    }

    pub(crate) fn is_held(&self) -> bool {
        self.state.recover_lock().holder.is_some()
    }

    /// Release only the acquisition identified by `token`. A stale token or a
    /// token belonging to another owner is a no-op.
    pub(crate) fn release(&self, token: CatalogLeaseToken) -> bool {
        let mut state = self.state.recover_lock();
        if state
            .holder
            .as_ref()
            .is_some_and(|holder| holder.token == token)
        {
            state.holder = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_owner_generation_can_release_the_catalog() {
        let lease = CatalogMutationLease::default();
        let recording = lease
            .try_acquire(CatalogLeaseOwner::Recording)
            .expect("recording acquires the empty catalog");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::DatasetImport)
                .is_none()
        );
        assert!(lease.release(recording));

        let dataset = lease
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .expect("dataset acquires after recording releases");
        assert!(
            !lease.release(recording),
            "a stale stop cannot clear import"
        );
        assert!(lease.is_held());
        assert!(lease.release(dataset));
        assert!(!lease.is_held());
    }

    #[test]
    fn short_lived_reservation_releases_on_scope_exit() {
        let lease = Arc::new(CatalogMutationLease::default());
        {
            let _reservation = lease
                .try_reserve(CatalogLeaseOwner::DatasetImport)
                .expect("the empty catalog can be reserved");
            assert!(lease.is_held());
        }
        assert!(!lease.is_held());
    }

    #[test]
    fn workflow_start_and_catalog_import_handshakes_are_linearized() {
        let lease = CatalogMutationLease::default();
        let import = lease
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .expect("import claims the empty handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none(),
            "an admitted import prevents a Workflow start"
        );
        assert!(lease.release(import));

        let workflow = lease
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .expect("Workflow claims the empty handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::DatasetImport)
                .is_none(),
            "import cannot pass its idle check before Workflow publishes active state"
        );
        assert!(lease.release(workflow));
        let import = lease
            .try_acquire(CatalogLeaseOwner::DatasetImport)
            .expect("import may perform its active-state check after Workflow releases handshake");
        assert!(lease.release(import));
    }

    #[test]
    fn assistant_admission_linearizes_with_manual_workflow_and_catalog_changes() {
        let lease = CatalogMutationLease::default();
        let agent = lease
            .try_acquire(CatalogLeaseOwner::StudioCatalogRead)
            .expect("Catalog read claims the empty handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none()
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::DatasetImport)
                .is_none()
        );
        assert!(lease.release(agent));

        let manual = lease
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .expect("Workflow Run claims the handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::StudioCatalogRead)
                .is_none()
        );
        assert!(lease.release(manual));

        let history = lease
            .try_acquire(CatalogLeaseOwner::StudioCatalogRead)
            .expect("Catalog read conversation persistence claims the handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::DatasetImport)
                .is_none(),
            "project import cannot replace history during conversation persistence"
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::DatasetImport)
                .is_none(),
            "destination cutover cannot replace history during conversation persistence"
        );
        assert!(lease.release(history));
    }
}
