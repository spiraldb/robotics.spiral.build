//! Exclusive, owner-scoped admission for episode-catalog publication.
//!
//! Recording start and dataset import allocate identities in the same catalog.
//! A boolean cannot express who owns that exclusion, so an unrelated stop can
//! accidentally release somebody else's mutation. This lease uses an opaque
//! generation token: only the exact acquisition may bind, claim, or release
//! its slot.

use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogLeaseOwner {
    Recording,
    DatasetImport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogLeaseToken {
    owner: CatalogLeaseOwner,
    generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaimedRecordingLease {
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
pub(crate) struct CatalogMutationLease {
    state: Mutex<LeaseState>,
}

impl CatalogMutationLease {
    pub(crate) fn try_acquire(&self, owner: CatalogLeaseOwner) -> Option<CatalogLeaseToken> {
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
    /// its still-held publication lease.
    pub(crate) fn bind_recording(&self, token: CatalogLeaseToken, episode: String) -> bool {
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
    /// durable staging calls [`release`](Self::release).
    pub(crate) fn claim_recording(&self) -> Option<ClaimedRecordingLease> {
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

    pub(crate) fn recording_episode(&self) -> Option<String> {
        let state = lock(&self.state);
        let holder = state.holder.as_ref()?;
        (holder.token.owner == CatalogLeaseOwner::Recording && !holder.claimed)
            .then(|| holder.recording_episode.clone())
            .flatten()
    }

    pub(crate) fn recording_is_opening(&self) -> bool {
        let state = lock(&self.state);
        state.holder.as_ref().is_some_and(|holder| {
            holder.token.owner == CatalogLeaseOwner::Recording
                && holder.recording_episode.is_none()
                && !holder.claimed
        })
    }

    pub(crate) fn is_held(&self) -> bool {
        lock(&self.state).holder.is_some()
    }

    /// Release only the acquisition identified by `token`. A stale token or a
    /// token belonging to another owner is a no-op.
    pub(crate) fn release(&self, token: CatalogLeaseToken) -> bool {
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
        assert!(lease.is_held(), "claim retains exclusion through staging");
        assert!(lease.release(claimed.token));
        assert!(!lease.release(claimed.token));
    }
}
