//! Episode catalog capability actions.
//!
//! This module owns which catalog mutations are allowed. Storage formats and
//! delivery response types remain opaque behind [`EpisodeCatalogPort`].

use crate::OperationError;
use spiral_robotics_api::identity::EpisodeId;

/// Current episode use that makes deletion unsafe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EpisodeActivity {
    recording: Option<String>,
    replaying: Option<String>,
}

impl EpisodeActivity {
    pub(crate) fn new(recording: Option<String>, replaying: Option<String>) -> Self {
        Self {
            recording,
            replaying,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeleteEpisodeCommand {
    episode: String,
}

impl DeleteEpisodeCommand {
    pub(crate) fn new(episode: impl Into<String>) -> Self {
        Self {
            episode: episode.into(),
        }
    }

    fn validated(mut self) -> Result<Self, OperationError> {
        self.episode = EpisodeId::try_from(self.episode.as_str())
            .map(String::from)
            .map_err(|_| {
                OperationError::invalid_input(
                    "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'",
                )
            })?;
        Ok(self)
    }
}

/// Consequences required by episode catalog actions.
///
pub(crate) trait EpisodeCatalogPort {
    fn activity(&self) -> EpisodeActivity;
    async fn delete_episode(&self, episode: String) -> Result<(), OperationError>;
}

pub(crate) struct EpisodeCatalogActions<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: EpisodeCatalogPort + ?Sized> EpisodeCatalogActions<'a, P> {
    pub(crate) fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub(crate) async fn delete(&self, command: DeleteEpisodeCommand) -> Result<(), OperationError> {
        let episode = command.validated()?.episode;
        let activity = self.port.activity();
        if activity.recording.as_deref() == Some(&episode) {
            return Err(OperationError::Conflict(format!(
                "take {episode:?} is still recording; stop it first"
            )));
        }
        if activity.replaying.as_deref() == Some(&episode) {
            return Err(OperationError::Conflict(format!(
                "take {episode:?} is driving an arm; stop replay before deleting it"
            )));
        }
        self.port.delete_episode(episode).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeCatalog {
        activity: EpisodeActivity,
        deleted: Mutex<Vec<String>>,
    }

    impl EpisodeCatalogPort for FakeCatalog {
        fn activity(&self) -> EpisodeActivity {
            self.activity.clone()
        }

        async fn delete_episode(&self, episode: String) -> Result<(), OperationError> {
            self.deleted.lock().expect("delete calls").push(episode);
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_active_recording_is_never_deleted() {
        let catalog = FakeCatalog {
            activity: EpisodeActivity {
                recording: Some("episode_000001".to_owned()),
                replaying: None,
            },
            ..FakeCatalog::default()
        };

        let error = EpisodeCatalogActions::new(&catalog)
            .delete(DeleteEpisodeCommand {
                episode: "episode_000001".to_owned(),
            })
            .await
            .expect_err("recording must block deletion");

        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(catalog.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_active_replay_is_never_deleted() {
        let catalog = FakeCatalog {
            activity: EpisodeActivity {
                recording: None,
                replaying: Some("episode_000002".to_owned()),
            },
            ..FakeCatalog::default()
        };

        let error = EpisodeCatalogActions::new(&catalog)
            .delete(DeleteEpisodeCommand {
                episode: "episode_000002".to_owned(),
            })
            .await
            .expect_err("replay must block deletion");

        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(catalog.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_inactive_episode_crosses_the_delete_boundary_once() {
        let catalog = FakeCatalog::default();

        EpisodeCatalogActions::new(&catalog)
            .delete(DeleteEpisodeCommand {
                episode: "episode_000003".to_owned(),
            })
            .await
            .unwrap();

        assert_eq!(
            catalog.deleted.lock().unwrap().as_slice(),
            ["episode_000003"]
        );
    }
}
