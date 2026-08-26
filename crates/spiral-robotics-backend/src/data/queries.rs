//! Read-only episode catalog use cases and delivery-neutral projections.

use crate::OperationError;
use spiral_robotics_api::identity::EpisodeId;
use spiral_robotics_api::recording::{EpisodeInfo, EpisodeInspectionSnapshot};

pub(crate) trait EpisodeQueryPort: Send + Sync {
    fn episodes(&self) -> Vec<EpisodeInfo>;
    fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError>;
    fn inspection(&self, episode: String) -> Result<EpisodeInspectionSnapshot, OperationError>;
}

pub(crate) struct EpisodeQueries<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: EpisodeQueryPort + ?Sized> EpisodeQueries<'a, P> {
    pub(crate) fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub(crate) fn authorize_asset(&self, episode: String) -> Result<(), OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        self.port.ensure_episode_exists(&episode)
    }

    pub(crate) fn list(&self) -> Vec<EpisodeInfo> {
        self.port.episodes()
    }

    pub(crate) fn inspection(
        &self,
        episode: String,
    ) -> Result<EpisodeInspectionSnapshot, OperationError> {
        let episode = EpisodeId::try_from(episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        self.port.inspection(episode)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Queries {
        calls: Mutex<Vec<String>>,
    }

    impl EpisodeQueryPort for Queries {
        fn episodes(&self) -> Vec<EpisodeInfo> {
            self.calls.lock().unwrap().push("list".to_owned());
            Vec::new()
        }

        fn ensure_episode_exists(&self, episode: &str) -> Result<(), OperationError> {
            self.calls.lock().unwrap().push(format!("exists:{episode}"));
            Ok(())
        }

        fn inspection(&self, episode: String) -> Result<EpisodeInspectionSnapshot, OperationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("inspection:{episode}"));
            Err(OperationError::NotFound(episode))
        }
    }

    #[test]
    fn read_queries_validate_and_bound_selection_before_exactly_one_port_call() {
        let queries = Queries::default();
        let episode_queries = EpisodeQueries::new(&queries);
        assert!(
            episode_queries
                .inspection("episode_000001".to_owned())
                .is_err()
        );
        assert!(episode_queries.inspection("../episode".to_owned()).is_err());
        episode_queries
            .authorize_asset("episode_000001".to_owned())
            .unwrap();
        assert!(
            episode_queries
                .authorize_asset("../episode".to_owned())
                .is_err()
        );
        assert!(
            episode_queries
                .authorize_asset(" episode".to_owned())
                .is_err()
        );
        assert_eq!(
            queries.calls.lock().unwrap().as_slice(),
            ["inspection:episode_000001", "exists:episode_000001"]
        );
    }
}
