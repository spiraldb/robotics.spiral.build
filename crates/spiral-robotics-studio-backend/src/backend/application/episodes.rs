//! Episode catalog application actions.
//!
//! This module owns which catalog mutations are allowed and which episodes an
//! export selects. Storage formats, task executors, video sidecars, and
//! delivery response types remain opaque behind [`EpisodeCatalogPort`].

use super::error::ApplicationError;

/// Current episode use that makes deletion unsafe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EpisodeActivity {
    pub recording: Option<String>,
    pub replaying: Option<String>,
}

/// The policy-relevant part of one catalog entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportCandidate {
    pub id: String,
    pub usable: bool,
    pub discarded: bool,
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

/// Export the current set of usable episodes accepted by the operator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ExportEpisodesCommand;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeMutationResult {
    pub episode: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ExportEpisodesResult {
    pub episodes: u64,
    pub frames: u64,
    pub skipped: u64,
}

/// Consequences required by episode catalog actions.
///
pub(crate) trait EpisodeCatalogPort {
    fn activity(&self) -> EpisodeActivity;
    fn export_candidates(&self) -> Result<Vec<ExportCandidate>, ApplicationError>;

    async fn judge_episode(
        &self,
        episode: String,
        discarded: bool,
    ) -> Result<EpisodeMutationResult, ApplicationError>;

    async fn delete_episode(&self, episode: String) -> Result<(), ApplicationError>;

    async fn export_episodes(
        &self,
        selected: Vec<String>,
    ) -> Result<ExportEpisodesResult, ApplicationError>;
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
/// storage adapter therefore cannot remove a recording file or a trajectory
/// that is currently driving an arm. Sidecar cleanup belongs inside the outer
/// adapter after its primary catalog deletion succeeds.
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

/// Select every usable, operator-kept take and export exactly that selection.
pub(crate) async fn export<P: EpisodeCatalogPort + ?Sized>(
    port: &P,
    _command: ExportEpisodesCommand,
) -> Result<ExportEpisodesResult, ApplicationError> {
    let candidates = port.export_candidates()?;
    let selected = candidates
        .into_iter()
        .filter(|candidate| candidate.usable && !candidate.discarded)
        .map(|candidate| candidate.id)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(ApplicationError::InvalidInput(
            "there are no kept takes to export yet".to_owned(),
        ));
    }
    port.export_episodes(selected).await
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
        candidates: Vec<ExportCandidate>,
        judged: Mutex<Vec<(String, bool)>>,
        deleted: Mutex<Vec<String>>,
        exported: Mutex<Vec<Vec<String>>>,
    }

    impl EpisodeCatalogPort for FakeCatalog {
        fn activity(&self) -> EpisodeActivity {
            self.activity.clone()
        }

        fn export_candidates(&self) -> Result<Vec<ExportCandidate>, ApplicationError> {
            Ok(self.candidates.clone())
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

        async fn export_episodes(
            &self,
            selected: Vec<String>,
        ) -> Result<ExportEpisodesResult, ApplicationError> {
            let count = selected.len();
            self.exported.lock().expect("export calls").push(selected);
            Ok(ExportEpisodesResult {
                episodes: count as u64,
                frames: 0,
                skipped: 0,
            })
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
    async fn export_selects_only_usable_operator_kept_takes() {
        let catalog = FakeCatalog {
            candidates: vec![
                ExportCandidate {
                    id: "kept".to_owned(),
                    usable: true,
                    discarded: false,
                },
                ExportCandidate {
                    id: "discarded".to_owned(),
                    usable: true,
                    discarded: true,
                },
                ExportCandidate {
                    id: "broken".to_owned(),
                    usable: false,
                    discarded: false,
                },
            ],
            ..FakeCatalog::default()
        };

        let result = export(&catalog, ExportEpisodesCommand).await.unwrap();

        assert_eq!(result.episodes, 1);
        assert_eq!(
            catalog.exported.lock().unwrap().as_slice(),
            [vec!["kept".to_owned()]]
        );
    }

    #[tokio::test]
    async fn export_refuses_before_calling_the_adapter_when_nothing_is_kept() {
        let catalog = FakeCatalog {
            candidates: vec![ExportCandidate {
                id: "discarded".to_owned(),
                usable: true,
                discarded: true,
            }],
            ..FakeCatalog::default()
        };

        let error = export(&catalog, ExportEpisodesCommand)
            .await
            .expect_err("an empty selection must be refused");

        assert!(matches!(error, ApplicationError::InvalidInput(_)));
        assert!(catalog.exported.lock().unwrap().is_empty());
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
