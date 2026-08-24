//! Exclusive, owner-scoped admission for episode-catalog mutation.
//!
//! Recording and calibration can mutate the same catalog.
//! A boolean cannot express who owns that exclusion, so an unrelated stop can
//! accidentally release somebody else's mutation. This lease uses an opaque
//! generation token: only the exact acquisition may bind, claim, or release
//! its slot.

use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogLeaseOwner {
    Recording,
    CalibrationReapply,
    EpisodeImport,
    DatasetImport,
    ProjectImport,
    ProjectSwitch,
    WorkflowStart,
    WorkflowCatalogRead,
    WorkflowAgentStart,
    WorkflowAgentHistory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogLeaseToken {
    owner: CatalogLeaseOwner,
    generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedRecordingLease {
    pub token: CatalogLeaseToken,
    pub episode: String,
}

struct LeaseHolder {
    token: CatalogLeaseToken,
    recording_episode: Option<String>,
    claimed: bool,
}

#[derive(Default)]
struct LeaseState {
    generation: u64,
    holder: Option<LeaseHolder>,
}

#[derive(Default)]
pub struct CatalogMutationLease {
    state: Mutex<LeaseState>,
}

impl CatalogMutationLease {
    pub fn try_acquire(&self, owner: CatalogLeaseOwner) -> Option<CatalogLeaseToken> {
        let mut state = lock(&self.state);
        if state.holder.is_some() {
            return None;
        }
        state.generation = state.generation.wrapping_add(1).max(1);
        let token = CatalogLeaseToken {
            owner,
            generation: state.generation,
        };
        state.holder = Some(LeaseHolder {
            token,
            recording_episode: None,
            claimed: false,
        });
        Some(token)
    }

    /// Associate the identity allocated by a successful recording start with
    /// its still-held catalog lease.
    pub fn bind_recording(&self, token: CatalogLeaseToken, episode: String) -> bool {
        let mut state = lock(&self.state);
        let Some(holder) = state.holder.as_mut() else {
            return false;
        };
        if holder.token != token
            || token.owner != CatalogLeaseOwner::Recording
            || holder.claimed
            || holder.recording_episode.is_some()
        {
            return false;
        }
        holder.recording_episode = Some(episode);
        true
    }

    /// Claim finalization exactly once while keeping the exclusion held until
    /// durable commit calls [`release`](Self::release).
    pub fn claim_recording(&self) -> Option<ClaimedRecordingLease> {
        let mut state = lock(&self.state);
        let holder = state.holder.as_mut()?;
        if holder.token.owner != CatalogLeaseOwner::Recording || holder.claimed {
            return None;
        }
        let episode = holder.recording_episode.clone()?;
        holder.claimed = true;
        Some(ClaimedRecordingLease {
            token: holder.token,
            episode,
        })
    }

    pub fn recording_episode(&self) -> Option<String> {
        let state = lock(&self.state);
        let holder = state.holder.as_ref()?;
        (holder.token.owner == CatalogLeaseOwner::Recording && !holder.claimed)
            .then(|| holder.recording_episode.clone())
            .flatten()
    }

    pub fn recording_is_opening(&self) -> bool {
        let state = lock(&self.state);
        state.holder.as_ref().is_some_and(|holder| {
            holder.token.owner == CatalogLeaseOwner::Recording
                && holder.recording_episode.is_none()
                && !holder.claimed
        })
    }

    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        lock(&self.state).holder.is_some()
    }

    /// Release only the acquisition identified by `token`. A stale token or a
    /// token belonging to another owner is a no-op.
    pub fn release(&self, token: CatalogLeaseToken) -> bool {
        let mut state = lock(&self.state);
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
                .try_acquire(CatalogLeaseOwner::CalibrationReapply)
                .is_none()
        );
        assert!(lease.release(recording));

        let dataset = lease
            .try_acquire(CatalogLeaseOwner::CalibrationReapply)
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
    fn a_camera_less_recording_can_be_claimed_and_released_exactly_once() {
        let lease = CatalogMutationLease::default();
        let token = lease
            .try_acquire(CatalogLeaseOwner::Recording)
            .expect("recording lease");
        assert!(lease.bind_recording(token, "episode_000001".to_owned()));
        assert_eq!(lease.recording_episode().as_deref(), Some("episode_000001"));

        let claimed = lease
            .claim_recording()
            .expect("automatic stop claims even without a media capture");
        assert_eq!(claimed.episode, "episode_000001");
        assert!(
            lease.claim_recording().is_none(),
            "finalization is exact once"
        );
        assert!(lease.is_held(), "claim retains exclusion through commit");
        assert!(lease.release(claimed.token));
        assert!(!lease.release(claimed.token));
    }

    #[test]
    fn workflow_start_and_catalog_import_handshakes_are_linearized() {
        let lease = CatalogMutationLease::default();
        let import = lease
            .try_acquire(CatalogLeaseOwner::ProjectImport)
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
                .try_acquire(CatalogLeaseOwner::EpisodeImport)
                .is_none(),
            "import cannot pass its idle check before Workflow publishes active state"
        );
        assert!(lease.release(workflow));
        let import = lease
            .try_acquire(CatalogLeaseOwner::EpisodeImport)
            .expect("import may perform its active-state check after Workflow releases handshake");
        assert!(lease.release(import));
    }

    #[test]
    fn assistant_admission_linearizes_with_manual_workflow_and_destination_changes() {
        let lease = CatalogMutationLease::default();
        let agent = lease
            .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
            .expect("Assistant claims the empty handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowStart)
                .is_none()
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::ProjectSwitch)
                .is_none()
        );
        assert!(lease.release(agent));

        let manual = lease
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .expect("manual Apply or Run claims the handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::WorkflowAgentStart)
                .is_none()
        );
        assert!(lease.release(manual));

        let history = lease
            .try_acquire(CatalogLeaseOwner::WorkflowAgentHistory)
            .expect("Assistant conversation persistence claims the handshake slot");
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::ProjectImport)
                .is_none(),
            "project import cannot replace history during conversation persistence"
        );
        assert!(
            lease
                .try_acquire(CatalogLeaseOwner::ProjectSwitch)
                .is_none(),
            "destination cutover cannot replace history during conversation persistence"
        );
        assert!(lease.release(history));
    }
}
