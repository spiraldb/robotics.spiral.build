//! Dataset import and export application actions.
//!
//! Archive formats and storage mechanics stay outside this module. This layer
//! admits dataset identities, selects operator-kept takes, proves that an
//! export can be represented without invented observations or actions, and
//! authorizes an import commit only after the entire source has been staged.

use spiral_robotics_core::identity::DatasetId;

use super::catalog_lease::CatalogLeaseToken;
use super::error::ApplicationError;

const MAX_DATASET_FPS: f64 = 1_000.0;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DatasetActivity {
    pub recording: Option<String>,
    pub catalog_mutation_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DatasetCandidate {
    pub id: String,
    pub dataset_id: String,
    pub usable: bool,
    pub discarded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExportEpisodeInspection {
    pub id: String,
    pub frames: u64,
    pub joints: Vec<String>,
    pub sample_hz: f64,
    pub complete_observations: bool,
    pub complete_actions: bool,
    pub complete_velocity: bool,
    pub complete_effort: bool,
}

pub(crate) struct InspectedDatasetExport<T> {
    pub prepared: T,
    pub episodes: Vec<ExportEpisodeInspection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportDatasetCommand {
    pub dataset_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportDatasetCommand {
    pub dataset_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportCompatibilityPlan {
    pub episode_ids: Vec<String>,
    pub joints: Vec<String>,
    pub fps: u32,
    pub include_proprioception: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DatasetExportSummary {
    pub episodes: u64,
    pub frames: u64,
    pub skipped: u64,
    pub without_proprioception: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DatasetImportSummary {
    pub episodes: u64,
    pub frames: u64,
    pub first_episode: Option<String>,
}

pub(crate) struct StagedDatasetImport<T> {
    pub staged: T,
    pub summary: DatasetImportSummary,
}

/// Consequences required by dataset interchange.
///
/// Prepared and staged values are adapter-minted capabilities consumed by the
/// same adapter after application policy authorizes the effect.
pub(crate) trait DatasetPort {
    type PreparedExport;
    type StagedImport;

    fn activity(&self) -> DatasetActivity;
    /// Atomically claim the catalog identity/publication boundary shared with
    /// recording start. The claim remains held through staged import commit.
    fn reserve_catalog_mutation(&self) -> Option<CatalogLeaseToken>;
    fn release_catalog_mutation(&self, token: CatalogLeaseToken);
    fn export_candidates(&self) -> Result<Vec<DatasetCandidate>, ApplicationError>;

    async fn inspect_export(
        &self,
        episode_ids: Vec<String>,
    ) -> Result<InspectedDatasetExport<Self::PreparedExport>, ApplicationError>;

    async fn write_export(
        &self,
        prepared: Self::PreparedExport,
        plan: ExportCompatibilityPlan,
    ) -> Result<(), ApplicationError>;

    async fn stage_import(
        &self,
        dataset_id: String,
    ) -> Result<StagedDatasetImport<Self::StagedImport>, ApplicationError>;

    async fn commit_import(&self, staged: Self::StagedImport) -> Result<(), ApplicationError>;
}

struct CatalogMutationReservation<'a, P: DatasetPort + ?Sized> {
    port: &'a P,
    token: CatalogLeaseToken,
}

impl<P: DatasetPort + ?Sized> Drop for CatalogMutationReservation<'_, P> {
    fn drop(&mut self) {
        self.port.release_catalog_mutation(self.token);
    }
}

pub(crate) async fn export<P: DatasetPort + ?Sized>(
    port: &P,
    command: ExportDatasetCommand,
) -> Result<DatasetExportSummary, ApplicationError> {
    let dataset_id = command.dataset_id.map(validate_dataset_id).transpose()?;
    ensure_idle(&port.activity(), "export")?;

    let candidates = port.export_candidates()?;
    ensure_unique_candidates(&candidates)?;
    let selected = candidates
        .iter()
        .filter(|candidate| {
            candidate.usable
                && !candidate.discarded
                && dataset_id
                    .as_ref()
                    .is_none_or(|dataset_id| candidate.dataset_id == *dataset_id)
        })
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(ApplicationError::InvalidInput(
            "there are no kept, complete takes in this dataset to export".to_owned(),
        ));
    }

    let inspected = port.inspect_export(selected.clone()).await?;
    let (plan, summary) = compatibility_plan(selected, candidates.len(), inspected.episodes)?;
    // A recording may have started while completed takes were being inspected.
    ensure_idle(&port.activity(), "export")?;
    port.write_export(inspected.prepared, plan).await?;
    Ok(summary)
}

pub(crate) async fn import<P: DatasetPort + ?Sized>(
    port: &P,
    command: ImportDatasetCommand,
) -> Result<DatasetImportSummary, ApplicationError> {
    let dataset_id = validate_dataset_id(command.dataset_id)?;
    let Some(token) = port.reserve_catalog_mutation() else {
        return Err(ApplicationError::Conflict(
            "another recording or dataset mutation is starting; wait for it to finish".to_owned(),
        ));
    };
    let _reservation = CatalogMutationReservation { port, token };
    // The reservation itself is a catalog mutation; only an already-active
    // recording can now conflict. A recording that races this point must lose
    // the same owner-scoped lease.
    if let Some(episode) = port.activity().recording {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is still recording; stop it before dataset import"
        )));
    }
    let staged = port.stage_import(dataset_id).await?;
    if staged.summary.episodes == 0 || staged.summary.frames == 0 {
        return Err(ApplicationError::InvalidInput(
            "the selected dataset contains no replayable episode frames".to_owned(),
        ));
    }
    port.commit_import(staged.staged).await?;
    Ok(staged.summary)
}

fn validate_dataset_id(value: String) -> Result<String, ApplicationError> {
    DatasetId::parse(&value)
        .map(DatasetId::into_string)
        .map_err(|error| ApplicationError::InvalidInput(error.to_string()))
}

fn ensure_idle(activity: &DatasetActivity, action: &str) -> Result<(), ApplicationError> {
    if let Some(episode) = &activity.recording {
        return Err(ApplicationError::Conflict(format!(
            "take {episode:?} is still recording; stop it before dataset {action}"
        )));
    }
    if activity.catalog_mutation_active {
        return Err(ApplicationError::Conflict(format!(
            "a recording or dataset mutation owns the catalog; wait for it to finish before dataset {action}"
        )));
    }
    Ok(())
}

fn ensure_unique_candidates(candidates: &[DatasetCandidate]) -> Result<(), ApplicationError> {
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.id.trim().is_empty()
            || candidates[..index]
                .iter()
                .any(|earlier| earlier.id == candidate.id)
        {
            return Err(ApplicationError::Internal(
                "the episode catalog returned empty or duplicate take identities".to_owned(),
            ));
        }
    }
    Ok(())
}

fn compatibility_plan(
    selected: Vec<String>,
    catalog_len: usize,
    inspected: Vec<ExportEpisodeInspection>,
) -> Result<(ExportCompatibilityPlan, DatasetExportSummary), ApplicationError> {
    let inspected_ids = inspected
        .iter()
        .map(|episode| episode.id.clone())
        .collect::<Vec<_>>();
    if inspected_ids != selected {
        return Err(ApplicationError::Conflict(
            "the episode catalog changed while preparing the dataset; retry the export".to_owned(),
        ));
    }
    let first = inspected
        .first()
        .ok_or_else(|| ApplicationError::InvalidInput("there are no takes to export".to_owned()))?;
    if first.joints.is_empty() {
        return Err(ApplicationError::InvalidInput(
            "an exported take has no follower joints".to_owned(),
        ));
    }
    let rounded_fps = first.sample_hz.round();
    if !first.sample_hz.is_finite()
        || first.sample_hz <= 0.0
        || !(1.0..=MAX_DATASET_FPS).contains(&rounded_fps)
    {
        return Err(ApplicationError::InvalidInput(
            "an exported take has an unusable sample rate".to_owned(),
        ));
    }

    let mut frames = 0_u64;
    let mut without_proprioception = 0_u64;
    for episode in &inspected {
        if episode.frames == 0 {
            return Err(ApplicationError::InvalidInput(format!(
                "take {:?} contains no frames to export",
                episode.id
            )));
        }
        if !episode.complete_observations {
            return Err(ApplicationError::InvalidInput(format!(
                "take {:?} has an incomplete follower observation; refusing to shorten it",
                episode.id
            )));
        }
        if !episode.complete_actions {
            return Err(ApplicationError::InvalidInput(format!(
                "take {:?} has no complete recorded action; refusing to fabricate a training label",
                episode.id
            )));
        }
        if episode.joints != first.joints {
            return Err(ApplicationError::InvalidInput(
                "the selected takes have different follower joint layouts".to_owned(),
            ));
        }
        if !episode.sample_hz.is_finite() || (episode.sample_hz - rounded_fps).abs() > 0.5 {
            return Err(ApplicationError::InvalidInput(
                "the selected takes have incompatible sample rates".to_owned(),
            ));
        }
        frames = frames.checked_add(episode.frames).ok_or_else(|| {
            ApplicationError::InvalidInput("the selected dataset is too large".to_owned())
        })?;
        if !episode.complete_velocity || !episode.complete_effort {
            without_proprioception += 1;
        }
    }
    let episodes = inspected.len() as u64;
    let skipped = catalog_len.saturating_sub(inspected.len()) as u64;
    let plan = ExportCompatibilityPlan {
        episode_ids: selected,
        joints: first.joints.clone(),
        fps: rounded_fps as u32,
        include_proprioception: without_proprioception == 0,
    };
    Ok((
        plan,
        DatasetExportSummary {
            episodes,
            frames,
            skipped,
            without_proprioception,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::backend::application::catalog_lease::{CatalogLeaseOwner, CatalogMutationLease};

    #[derive(Default)]
    struct FakeDatasetPort {
        activity: Mutex<DatasetActivity>,
        candidates: Vec<DatasetCandidate>,
        inspections: Vec<ExportEpisodeInspection>,
        stage_summary: Option<DatasetImportSummary>,
        stage_error: Option<ApplicationError>,
        calls: Mutex<Vec<String>>,
        catalog: CatalogMutationLease,
        catalog_unavailable: bool,
    }

    impl FakeDatasetPort {
        fn note(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_owned());
        }
    }

    impl DatasetPort for FakeDatasetPort {
        type PreparedExport = String;
        type StagedImport = String;

        fn activity(&self) -> DatasetActivity {
            self.note("activity");
            self.activity.lock().unwrap().clone()
        }

        fn reserve_catalog_mutation(&self) -> Option<CatalogLeaseToken> {
            self.note("reserve");
            (!self.catalog_unavailable)
                .then(|| self.catalog.try_acquire(CatalogLeaseOwner::DatasetImport))
                .flatten()
        }

        fn release_catalog_mutation(&self, token: CatalogLeaseToken) {
            self.note("release");
            assert!(self.catalog.release(token));
        }

        fn export_candidates(&self) -> Result<Vec<DatasetCandidate>, ApplicationError> {
            self.note("candidates");
            Ok(self.candidates.clone())
        }

        async fn inspect_export(
            &self,
            episode_ids: Vec<String>,
        ) -> Result<InspectedDatasetExport<Self::PreparedExport>, ApplicationError> {
            self.note(&format!("inspect:{}", episode_ids.join(",")));
            Ok(InspectedDatasetExport {
                prepared: "prepared".to_owned(),
                episodes: self.inspections.clone(),
            })
        }

        async fn write_export(
            &self,
            prepared: Self::PreparedExport,
            plan: ExportCompatibilityPlan,
        ) -> Result<(), ApplicationError> {
            self.note(&format!(
                "write:{prepared}:{}:{}",
                plan.episode_ids.join(","),
                plan.include_proprioception
            ));
            Ok(())
        }

        async fn stage_import(
            &self,
            dataset_id: String,
        ) -> Result<StagedDatasetImport<Self::StagedImport>, ApplicationError> {
            self.note(&format!("stage:{dataset_id}"));
            if let Some(error) = &self.stage_error {
                return Err(error.clone());
            }
            Ok(StagedDatasetImport {
                staged: "staged".to_owned(),
                summary: self.stage_summary.clone().unwrap(),
            })
        }

        async fn commit_import(&self, staged: Self::StagedImport) -> Result<(), ApplicationError> {
            self.note(&format!("commit:{staged}"));
            Ok(())
        }
    }

    fn candidate(id: &str, dataset_id: &str) -> DatasetCandidate {
        DatasetCandidate {
            id: id.to_owned(),
            dataset_id: dataset_id.to_owned(),
            usable: true,
            discarded: false,
        }
    }

    fn inspection(id: &str) -> ExportEpisodeInspection {
        ExportEpisodeInspection {
            id: id.to_owned(),
            frames: 12,
            joints: vec!["shoulder".to_owned()],
            sample_hz: 60.0,
            complete_observations: true,
            complete_actions: true,
            complete_velocity: true,
            complete_effort: true,
        }
    }

    #[tokio::test]
    async fn invalid_destination_and_active_recording_call_no_archive_adapter() {
        let port = FakeDatasetPort::default();
        let error = export(
            &port,
            ExportDatasetCommand {
                dataset_id: Some("../escape".to_owned()),
            },
        )
        .await
        .expect_err("invalid ID");
        assert!(matches!(error, ApplicationError::InvalidInput(_)));
        assert!(port.calls.lock().unwrap().is_empty());

        *port.activity.lock().unwrap() = DatasetActivity {
            recording: Some("episode_000001".to_owned()),
            catalog_mutation_active: false,
        };
        let error = import(
            &port,
            ImportDatasetCommand {
                dataset_id: "default".to_owned(),
            },
        )
        .await
        .expect_err("recording conflict");
        assert!(matches!(error, ApplicationError::Conflict(_)));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["reserve", "activity", "release"]
        );
    }

    #[tokio::test]
    async fn export_orders_policy_checks_and_never_invents_optional_columns() {
        let port = FakeDatasetPort {
            candidates: vec![candidate("first", "pick"), candidate("second", "pick")],
            inspections: vec![inspection("first"), {
                let mut second = inspection("second");
                second.complete_effort = false;
                second
            }],
            ..FakeDatasetPort::default()
        };
        let result = export(
            &port,
            ExportDatasetCommand {
                dataset_id: Some("pick".to_owned()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.episodes, 2);
        assert_eq!(result.without_proprioception, 1);
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "activity",
                "candidates",
                "inspect:first,second",
                "activity",
                "write:prepared:first,second:false",
            ]
        );
    }

    #[tokio::test]
    async fn missing_action_refuses_before_archive_write() {
        let mut broken = inspection("broken");
        broken.complete_actions = false;
        let port = FakeDatasetPort {
            candidates: vec![candidate("broken", "default")],
            inspections: vec![broken],
            ..FakeDatasetPort::default()
        };

        let error = export(&port, ExportDatasetCommand { dataset_id: None })
            .await
            .expect_err("missing labels must be rejected");
        assert!(error.to_string().contains("fabricate"));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["activity", "candidates", "inspect:broken"]
        );
    }

    #[tokio::test]
    async fn stage_failure_never_commits_and_success_commits_last() {
        let failed = FakeDatasetPort {
            stage_error: Some(ApplicationError::InvalidInput("bad archive".to_owned())),
            ..FakeDatasetPort::default()
        };
        let _ = import(
            &failed,
            ImportDatasetCommand {
                dataset_id: "default".to_owned(),
            },
        )
        .await
        .expect_err("stage failure");
        assert_eq!(
            failed.calls.lock().unwrap().as_slice(),
            ["reserve", "activity", "stage:default", "release"]
        );

        let success = FakeDatasetPort {
            stage_summary: Some(DatasetImportSummary {
                episodes: 2,
                frames: 24,
                first_episode: Some("episode_000010".to_owned()),
            }),
            ..FakeDatasetPort::default()
        };
        let summary = import(
            &success,
            ImportDatasetCommand {
                dataset_id: " imported ".to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(summary.episodes, 2);
        assert_eq!(
            success.calls.lock().unwrap().as_slice(),
            [
                "reserve",
                "activity",
                "stage:imported",
                "commit:staged",
                "release"
            ]
        );
    }

    #[tokio::test]
    async fn catalog_claim_blocks_import_before_any_adapter_io() {
        let port = FakeDatasetPort {
            stage_summary: Some(DatasetImportSummary {
                episodes: 1,
                frames: 1,
                first_episode: None,
            }),
            catalog_unavailable: true,
            ..FakeDatasetPort::default()
        };
        let error = import(
            &port,
            ImportDatasetCommand {
                dataset_id: "default".to_owned(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ApplicationError::Conflict(_)));
        assert_eq!(port.calls.lock().unwrap().as_slice(), ["reserve"]);
    }
}
