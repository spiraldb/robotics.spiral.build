//! Episode catalog application actions.
//!
//! This module owns which catalog mutations are allowed. Storage formats and
//! delivery response types remain opaque behind [`EpisodeCatalogPort`].

use super::error::ApplicationError;

/// Current episode use that makes deletion unsafe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EpisodeActivity {
    pub recording: Option<String>,
    pub replaying: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JudgeEpisodeCommand {
    pub episode: String,
    pub discarded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeleteEpisodeCommand {
    pub episode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeMutationResult {
    pub episode: String,
}

/// Consequences required by episode catalog actions.
///
pub(crate) trait EpisodeCatalogPort {
    fn activity(&self) -> EpisodeActivity;
    async fn judge_episode(
        &self,
        episode: String,
        discarded: bool,
    ) -> Result<EpisodeMutationResult, ApplicationError>;

    async fn delete_episode(&self, episode: String) -> Result<(), ApplicationError>;
}

pub(crate) async fn judge<P: EpisodeCatalogPort + ?Sized>(
    port: &P,
    command: JudgeEpisodeCommand,
) -> Result<EpisodeMutationResult, ApplicationError> {
    let episode = validate_episode_id(command.episode)?;
    port.judge_episode(episode, command.discarded).await
}

/// Delete one inactive episode.
///
/// The activity checks deliberately precede the deletion boundary call. A
/// storage adapter therefore cannot remove a recording or a trajectory that is
/// currently driving an arm. Related-row cleanup belongs inside the repository
/// transaction.
pub(crate) async fn delete<P: EpisodeCatalogPort + ?Sized>(
    port: &P,
    command: DeleteEpisodeCommand,
) -> Result<(), ApplicationError> {
    let episode = validate_episode_id(command.episode)?;
    let activity = port.activity();
    if activity.recording.as_deref() == Some(&episode) {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is still recording; stop it first"
        )));
    }
    if activity.replaying.as_deref() == Some(&episode) {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is driving an arm; stop replay before deleting it"
        )));
    }
    port.delete_episode(episode).await
}

fn validate_episode_id(episode: String) -> Result<String, ApplicationError> {
    if episode.is_empty()
        || episode.len() > 64
        || !episode
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApplicationError::InvalidInput(
            "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'".to_owned(),
        ));
    }
    Ok(episode)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeCatalog {
        activity: EpisodeActivity,
        judged: Mutex<Vec<(String, bool)>>,
        deleted: Mutex<Vec<String>>,
    }

    impl EpisodeCatalogPort for FakeCatalog {
        fn activity(&self) -> EpisodeActivity {
            self.activity.clone()
        }

        async fn judge_episode(
            &self,
            episode: String,
            discarded: bool,
        ) -> Result<EpisodeMutationResult, ApplicationError> {
            self.judged
                .lock()
                .expect("judgement calls")
                .push((episode.clone(), discarded));
            Ok(EpisodeMutationResult { episode })
        }

        async fn delete_episode(&self, episode: String) -> Result<(), ApplicationError> {
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

        let error = delete(
            &catalog,
            DeleteEpisodeCommand {
                episode: "episode_000001".to_owned(),
            },
        )
        .await
        .expect_err("recording must block deletion");

        assert!(matches!(error, ApplicationError::Conflict(_)));
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

        let error = delete(
            &catalog,
            DeleteEpisodeCommand {
                episode: "episode_000002".to_owned(),
            },
        )
        .await
        .expect_err("replay must block deletion");

        assert!(matches!(error, ApplicationError::Conflict(_)));
        assert!(catalog.deleted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_inactive_episode_crosses_the_delete_boundary_once() {
        let catalog = FakeCatalog::default();

        delete(
            &catalog,
            DeleteEpisodeCommand {
                episode: "episode_000003".to_owned(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            catalog.deleted.lock().unwrap().as_slice(),
            ["episode_000003"]
        );
    }

    #[tokio::test]
    async fn judging_returns_the_adapter_output() {
        let catalog = FakeCatalog::default();
        let output = judge(
            &catalog,
            JudgeEpisodeCommand {
                episode: "episode_000004".to_owned(),
                discarded: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(output.episode, "episode_000004");
        assert_eq!(
            catalog.judged.lock().unwrap().as_slice(),
            [("episode_000004".to_owned(), true)]
        );
    }
}
