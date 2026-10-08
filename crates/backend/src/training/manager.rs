//! One project's training runs: Studio's record of each run it started,
//! joined with where the run is now — in Studio's upload while its data goes
//! to Modal, and on the Modal training service after that.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spiral_robotics_common::training::{
    CheckpointModel, CheckpointPolicy, ModelPublication, PublishedModel, TrainingCheckpoint,
    TrainingConfig, TrainingModel, TrainingReadiness, TrainingRun, TrainingRunIdentity,
    TrainingRunStatus, TrainingSnapshot,
};
use spiral_robotics_inference::ModelArtifact;
use spiral_robotics_storage::{ProviderKeys, StorageManager};

use super::admission::{ModelPublishing, RunDeletion, StartTrainingAdmission as _};
use super::input::TrainingInput;
use super::service::{ModelPublishRequest, RunRequest, RunStatus, TrainingService};
use super::upload::{RunUploads, UploadWork};
use crate::OperationError;
use crate::clock::StableIdSequence;
use crate::sync::MutexRecovery;

/// How long a probe of the training service answers for whether a run could
/// start.
const READINESS_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingRuntime>,
}

struct TrainingRuntime {
    storage: Arc<StorageManager>,
    keys: Arc<ProviderKeys>,
    run_ids: StableIdSequence,
    uploads: RunUploads,
    /// The last probe of the training service and when it was taken.
    readiness: Mutex<Option<(Instant, TrainingReadiness)>>,
}

impl TrainingManager {
    /// Construct the manager without touching external services.
    pub(crate) fn new(storage: Arc<StorageManager>, keys: Arc<ProviderKeys>) -> Self {
        Self {
            inner: Arc::new(TrainingRuntime {
                storage,
                keys,
                run_ids: StableIdSequence::new("train", 0),
                uploads: RunUploads::default(),
                readiness: Mutex::new(None),
            }),
        }
    }

    /// Every recorded run, newest first, and whether a new one could start.
    pub(super) async fn catalog(&self) -> Result<TrainingSnapshot, OperationError> {
        let runs = self.reports().await?.runs();
        Ok(TrainingSnapshot::new(self.readiness().await, runs))
    }

    /// Whether any recorded run is still active.
    pub(crate) async fn active(&self) -> Result<bool, OperationError> {
        Ok(self
            .reports()
            .await?
            .runs()
            .iter()
            .any(|run| run.phase().active()))
    }

    /// One run with the metrics and log tail its job keeps.
    pub(super) async fn run(&self, id: &str) -> Result<TrainingRun, OperationError> {
        let identity = self.record(id).await?;
        if let Some(status) = self.inner.uploads.status(id) {
            return Ok(TrainingRun::new(identity, status));
        }
        let history = match self.service() {
            Ok(service) => service.history(&self.project(), id).await,
            Err(error) => Err(error),
        };
        Ok(match history {
            Ok(Some(history)) => {
                let (status, history) = history.into_parts();
                TrainingRun::new(identity, status.status().clone()).with_history(history)
            }
            Ok(None) => TrainingRun::new(identity, RunReports::never_arrived()),
            Err(error) => TrainingRun::new(identity, TrainingRunStatus::unavailable(error)),
        })
    }

    /// Every checkpoint any run saved.
    pub(super) async fn checkpoints(&self) -> Result<Vec<TrainingCheckpoint>, OperationError> {
        Ok(self.reports().await?.checkpoints())
    }

    /// Checkpoint `step` of `run_id` as a policy, for the robot connector to
    /// roll out. Loading it is what proves it exists.
    pub(super) async fn policy(
        &self,
        run_id: &str,
        step: u64,
    ) -> Result<ModelArtifact, OperationError> {
        if !matches!(
            self.record(run_id).await?.config().model(),
            TrainingModel::Act(_)
        ) {
            return Err(OperationError::invalid_input(format!(
                "Training run {run_id:?} trained no policy to roll out."
            )));
        }
        let (config, weights) = self
            .service()
            .map_err(OperationError::External)?
            .checkpoint(&self.project(), run_id, step)
            .await
            .map_err(OperationError::External)?;
        ModelArtifact::from_bytes(config, weights)
            .map_err(|error| OperationError::Internal(error.to_string()))
    }

    /// Publish the checkpoint `publication` names under its name, so Score
    /// progress can score with it: the service keeps the weights, and Studio
    /// the name. The same weights may be published under another name.
    pub(super) async fn publish(
        &self,
        publication: ModelPublication,
    ) -> Result<PublishedModel, OperationError> {
        let run = self.record(publication.run_id()).await?;
        let service = self.service().map_err(OperationError::External)?;
        let project = self.project();
        let statuses = service
            .statuses(&project, &[run.id().to_owned()])
            .await
            .map_err(OperationError::External)?;
        let status = statuses.iter().find(|status| status.run_id() == run.id());
        let publication = ModelPublishing::admitted(publication, &run, status)?;
        let request = ModelPublishRequest::new(&project, run.id(), publication.step());
        let model = service
            .publish(&request)
            .await
            .map_err(OperationError::External)?
            .named(publication);
        let recorded = self
            .inner
            .storage
            .published_models()
            .insert(&model)
            .await
            .map_err(OperationError::External)?;
        if !recorded {
            return Err(OperationError::conflict(format!(
                "A model named “{}” is already published; choose another name.",
                model.name()
            )));
        }
        Ok(model)
    }

    /// Every progress model the project published, by name.
    pub(super) async fn published_models(&self) -> Result<Vec<PublishedModel>, OperationError> {
        self.inner
            .storage
            .published_models()
            .list()
            .await
            .map_err(OperationError::External)
    }

    /// Admit `config` as a run, record it, and start uploading its data.
    pub(super) async fn start(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, OperationError> {
        let config = config.validated()?;
        let service = self.service().map_err(OperationError::Conflict)?;
        service.health().await.map_err(OperationError::Conflict)?;
        if let Some(active) = self
            .reports()
            .await?
            .runs()
            .into_iter()
            .find(|run| run.phase().active())
        {
            return Err(OperationError::Conflict(format!(
                "Training run {:?} is still active; stop it before starting another.",
                active.config().run_name()
            )));
        }
        let input = TrainingInput::resolve(&self.inner.storage, &config).await?;
        let (id, started_ms) = self.inner.run_ids.next().into_parts();
        let identity =
            TrainingRunIdentity::new(id.clone(), config.clone(), input.summary(), started_ms);
        self.inner
            .storage
            .training_runs()
            .insert(&identity)
            .await
            .map_err(OperationError::External)?;
        let request = RunRequest::new((&self.project(), &id), config, input.run_data());
        self.inner.uploads.start(
            id.clone(),
            UploadWork::new(
                (self.inner.storage.clone(), service),
                input.streams(),
                request,
            ),
        );
        let status = self
            .inner
            .uploads
            .status(&id)
            .unwrap_or_else(RunReports::never_arrived);
        Ok(TrainingRun::new(identity, status))
    }

    /// Stop run `run_id`: its upload, or its job.
    pub(super) async fn stop(&self, run_id: &str) -> Result<(), OperationError> {
        self.record(run_id).await?;
        if self.inner.uploads.stop(run_id) {
            return Ok(());
        }
        self.service()
            .map_err(OperationError::External)?
            .stop(&self.project(), run_id)
            .await
            .map(drop)
            .map_err(OperationError::External)
    }

    /// Delete a settled run: its checkpoints on Modal, then its record, so a
    /// failed deletion leaves the run visible and retryable.
    pub(super) async fn delete(&self, run_id: &str) -> Result<(), OperationError> {
        let run_id = RunDeletion::admitted(&self.reports().await?.runs(), run_id)?;
        self.service()
            .map_err(OperationError::External)?
            .delete(&self.project(), &run_id)
            .await
            .map_err(OperationError::External)?;
        self.inner.uploads.forget(&run_id);
        self.inner
            .storage
            .training_runs()
            .delete(&run_id)
            .await
            .map_err(OperationError::External)
    }

    async fn record(&self, id: &str) -> Result<TrainingRunIdentity, OperationError> {
        self.inner
            .storage
            .training_runs()
            .load_run(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| OperationError::NotFound(format!("Training run {id:?} was not found")))
    }

    /// Every recorded run beside what Studio's uploads and the service report
    /// about it, asking the service once.
    async fn reports(&self) -> Result<RunReports, OperationError> {
        let records = self
            .inner
            .storage
            .training_runs()
            .load()
            .await
            .map_err(OperationError::External)?;
        let uploading = records
            .iter()
            .filter_map(|record| {
                let status = self.inner.uploads.status(record.id())?;
                Some((record.id().to_owned(), status))
            })
            .collect::<BTreeMap<_, _>>();
        let asked = records
            .iter()
            .map(|record| record.id().to_owned())
            .filter(|id| !uploading.contains_key(id))
            .collect::<Vec<_>>();
        let service = match asked.is_empty() {
            true => Ok(Vec::new()),
            false => match self.service() {
                Ok(service) => service.statuses(&self.project(), &asked).await,
                Err(error) => Err(error),
            },
        };
        Ok(RunReports {
            records,
            uploading,
            service,
        })
    }

    /// Whether a run could start now, probed at most every half minute.
    async fn readiness(&self) -> TrainingReadiness {
        if let Some((probed, readiness)) = self.inner.readiness.recover_lock().clone()
            && probed.elapsed() < READINESS_LIFETIME
        {
            return readiness;
        }
        let readiness = match self.service() {
            Ok(service) => match service.health().await {
                Ok(gpu) => TrainingReadiness::new(true, format!("Training on Modal · {gpu}")),
                Err(error) => TrainingReadiness::new(false, error),
            },
            Err(error) => TrainingReadiness::new(false, error),
        };
        *self.inner.readiness.recover_lock() = Some((Instant::now(), readiness.clone()));
        readiness
    }

    fn service(&self) -> Result<TrainingService, String> {
        TrainingService::of(&self.inner.keys)
    }

    /// The Spiral project these runs belong to, which names them on Modal.
    fn project(&self) -> String {
        self.inner.storage.project_store().project_id().to_owned()
    }
}

/// Every recorded run and what reports where each is.
struct RunReports {
    records: Vec<TrainingRunIdentity>,
    /// Runs Studio is still uploading, or whose upload ended them.
    uploading: BTreeMap<String, TrainingRunStatus>,
    /// What the service reports about the rest, or why it cannot.
    service: Result<Vec<RunStatus>, String>,
}

impl RunReports {
    /// A run neither Studio nor the service holds.
    fn never_arrived() -> TrainingRunStatus {
        TrainingRunStatus::unavailable(
            "This run never reached Modal: Studio restarted while uploading its data. Start it again; only what is missing uploads.",
        )
    }

    fn runs(&self) -> Vec<TrainingRun> {
        self.records
            .iter()
            .map(|record| TrainingRun::new(record.clone(), self.status(record.id())))
            .collect()
    }

    fn status(&self, id: &str) -> TrainingRunStatus {
        if let Some(status) = self.uploading.get(id) {
            return status.clone();
        }
        match &self.service {
            Ok(statuses) => statuses
                .iter()
                .find(|status| status.run_id() == id)
                .map_or_else(Self::never_arrived, |status| status.status().clone()),
            Err(error) => TrainingRunStatus::unavailable(error.clone()),
        }
    }

    /// Every checkpoint the service reports, with its model's family and,
    /// for a policy, what it expects.
    fn checkpoints(&self) -> Vec<TrainingCheckpoint> {
        let Ok(statuses) = &self.service else {
            return Vec::new();
        };
        statuses
            .iter()
            .filter_map(|status| {
                let record = self
                    .records
                    .iter()
                    .find(|record| record.id() == status.run_id())?;
                let model = match record.config().model() {
                    TrainingModel::Act(act) => CheckpointModel::Act(CheckpointPolicy::new(
                        act.observation().cameras().to_vec(),
                        act.chunk_size(),
                        Some(act.observation().rate_hz()),
                    )),
                    TrainingModel::Progress(_) => CheckpointModel::Progress,
                };
                Some(status.checkpoints().iter().map(move |checkpoint| {
                    TrainingCheckpoint::new(status.run_id(), checkpoint.step(), model.clone())
                        .with_validation_loss(checkpoint.validation_loss())
                }))
            })
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests;
