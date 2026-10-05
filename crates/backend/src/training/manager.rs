//! One project's training runs: Studio's record of each run it started,
//! joined with what the run's worker reports now. Workers own execution,
//! metrics, and logs; checkpoints live in Spiral.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spiral_robotics_common::training::{
    TrainingCheckpoint, TrainingConfig, TrainingReadiness, TrainingRun, TrainingRunIdentity,
    TrainingRunStatus, TrainingRuntimeInfo, TrainingSnapshot,
};
use spiral_robotics_inference::ModelArtifact;
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::checkpoints::CheckpointKey;
use spiral_robotics_storage::training_runs::TrainingRunRecord;
use spiral_robotics_training::{
    SpiralJobConnection, SpiralServiceEndpoints, TrainingBackendInfo, TrainingJobRequest,
};

use crate::OperationError;
use crate::adapters::compute_runtime::HttpTrainingBackend;
use crate::clock::StableIdSequence;
use crate::sync::MutexRecovery;
use crate::training::service::StartTrainingAdmission as _;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(crate) struct TrainingManager {
    inner: Arc<TrainingRuntime>,
}

struct TrainingRuntime {
    storage: Arc<StorageManager>,
    run_ids: StableIdSequence,
    /// The last probe of the training runtime and Spiral, a live service
    /// state rather than history.
    readiness: Mutex<Option<TrainingReadiness>>,
}

impl TrainingManager {
    /// Construct the manager without touching external services.
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            inner: Arc::new(TrainingRuntime {
                storage,
                run_ids: StableIdSequence::new("train", 0),
                readiness: Mutex::new(None),
            }),
        }
    }

    /// Every recorded run, newest first, as its worker reports it now.
    pub(super) async fn runs(&self) -> Result<Vec<TrainingRun>, OperationError> {
        let records = self
            .inner
            .storage
            .training_runs()
            .load()
            .await
            .map_err(OperationError::External)?;
        let workers = WorkerJobs::fetch(&records).await;
        Ok(records.iter().map(|record| workers.run(record)).collect())
    }

    /// Whether any recorded run is still active on its worker.
    pub(crate) async fn active(&self) -> Result<bool, OperationError> {
        Ok(self.runs().await?.iter().any(|run| run.phase().active()))
    }

    pub(super) async fn catalog(&self) -> Result<TrainingSnapshot, OperationError> {
        Ok(TrainingSnapshot::new(
            self.readiness().await,
            self.runs().await?,
        ))
    }

    /// One run together with the metrics and log tail its worker retains.
    pub(super) async fn run(&self, id: &str) -> Result<TrainingRun, OperationError> {
        let record = self.record(id).await?;
        let (endpoint, job_id) = (record.runtime_endpoint().to_owned(), id.to_owned());
        let history =
            HttpTrainingBackend::request(endpoint.clone(), move |worker| worker.job(&job_id)).await;
        Ok(match history {
            Ok(history) => {
                let (state, history) = history.into_parts();
                TrainingRun::new(record.identity().clone(), state.into_status())
                    .with_history(history)
            }
            Err(error) => TrainingRun::new(
                record.identity().clone(),
                TrainingRunStatus::unavailable(format!(
                    "The training runtime at {endpoint} cannot report this run: {error}"
                )),
            ),
        })
    }

    /// Every checkpoint any run committed to Spiral.
    /// Checkpoint `step` of `run_id` as a policy, for the robot connector to
    /// roll out. Loading it is what proves it exists.
    pub(super) async fn policy(
        &self,
        run_id: &str,
        step: u64,
    ) -> Result<ModelArtifact, OperationError> {
        let (config, weights) = self
            .inner
            .storage
            .training_checkpoints()
            .load(CheckpointKey::new(run_id.to_owned(), step))
            .await
            .map_err(OperationError::External)?
            .into_parts();
        ModelArtifact::from_bytes(config, weights)
            .map_err(|error| OperationError::Internal(error.to_string()))
    }

    pub(super) async fn checkpoints(&self) -> Result<Vec<TrainingCheckpoint>, OperationError> {
        self.inner
            .storage
            .training_checkpoints()
            .list()
            .await
            .map_err(OperationError::External)
    }

    /// Start one run on the runtime Settings selects now. The run keeps that
    /// runtime for its whole life.
    pub(super) async fn start(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, OperationError> {
        if let Some(reason) = config.policy().mcap_training_blocker() {
            return Err(OperationError::Conflict(reason.into()));
        }
        let config = config.validated()?;
        let readiness = self.refresh_readiness().await;
        if !readiness.ready() {
            return Err(OperationError::Conflict(readiness.summary().to_owned()));
        }
        if let Some(active) = self.runs().await?.iter().find(|run| run.phase().active()) {
            return Err(OperationError::Conflict(format!(
                "training run {:?} is already active; stop it before starting another",
                active.id()
            )));
        }
        let endpoint = self
            .runtime_endpoint()
            .await
            .map_err(OperationError::External)?;
        let (id, started_ms) = self.inner.run_ids.next().into_parts();
        let request = TrainingJobRequest::new(id.clone(), self.spiral_connection(), config.clone());
        let state = HttpTrainingBackend::request(endpoint.clone(), move |worker| {
            worker.create_job(&request)
        })
        .await
        .map_err(OperationError::External)?;
        let identity = TrainingRunIdentity::new(id, config, started_ms);
        self.inner
            .storage
            .training_runs()
            .insert(&TrainingRunRecord::new(identity.clone(), endpoint))
            .await
            .map_err(OperationError::External)?;
        Ok(TrainingRun::new(identity, state.into_status()))
    }

    /// Ask the run's worker to stop it.
    pub(super) async fn stop(&self, run_id: &str) -> Result<(), OperationError> {
        let record = self.record(run_id).await?;
        let job_id = run_id.to_owned();
        HttpTrainingBackend::request(record.runtime_endpoint().to_owned(), move |worker| {
            worker.stop_job(&job_id)
        })
        .await
        .map(|_| ())
        .map_err(OperationError::External)
    }

    /// Remove checkpoints before the record, so a failed checkpoint deletion
    /// leaves the run visible and retryable instead of orphaning its models.
    pub(super) async fn delete(&self, run_id: &str) -> Result<(), OperationError> {
        let run_id = Self::admitted_deletion(&self.runs().await?, run_id)?;
        self.inner
            .storage
            .training_checkpoints()
            .delete_run(&run_id)
            .await
            .map_err(OperationError::External)?;
        self.inner
            .storage
            .training_runs()
            .delete(&run_id)
            .await
            .map_err(OperationError::External)
    }

    async fn record(&self, id: &str) -> Result<TrainingRunRecord, OperationError> {
        self.inner
            .storage
            .training_runs()
            .load_run(id)
            .await
            .map_err(OperationError::External)?
            .ok_or_else(|| OperationError::NotFound(format!("Training run {id:?} was not found")))
    }

    fn spiral_connection(&self) -> SpiralJobConnection {
        let project = self.inner.storage.project_store();
        SpiralJobConnection::new(
            SpiralServiceEndpoints::new(project.endpoint(), project.spfs_endpoint()),
            project.project_id(),
            spiral_robotics_storage::STUDIO_TABLE_PREFIX,
        )
    }

    /// The last probe, or a first one.
    async fn readiness(&self) -> TrainingReadiness {
        let known = self.inner.readiness.recover_lock().clone();
        match known {
            Some(readiness) => readiness,
            None => self.refresh_readiness().await,
        }
    }

    pub(crate) async fn refresh_readiness(&self) -> TrainingReadiness {
        let next = self.probe_readiness().await;
        *self.inner.readiness.recover_lock() = Some(next.clone());
        next
    }

    pub(crate) async fn test_runtime(&self) -> (bool, String) {
        match self.probe_backend().await {
            Ok(info) => (true, format!("Connected · {}", info.display_summary())),
            Err(error) => (false, error),
        }
    }

    async fn probe_readiness(&self) -> TrainingReadiness {
        let backend_probe = self.probe_backend();
        let spiral_probe = async {
            tokio::time::timeout(PROBE_TIMEOUT, self.inner.storage.database().test())
                .await
                .unwrap_or_else(|_| {
                    (
                        false,
                        "the connection probe timed out after 8 seconds".to_owned(),
                    )
                })
        };
        let (backend, connection) = tokio::join!(backend_probe, spiral_probe);
        let runtime = backend.as_ref().ok().map(|info| {
            TrainingRuntimeInfo::new(info.display_name(), info.runtime())
                .with_devices(info.devices().to_vec())
        });
        let spiral_ready = connection.0;
        let blocker = spiral_robotics_common::training::TrainingPolicy::Act.mcap_training_blocker();
        let ready = spiral_ready && backend.is_ok() && blocker.is_none();
        let summary = match (connection, backend, blocker) {
            ((false, message), _, _) => format!("Spiral is not ready for training: {message}"),
            (_, Err(error), _) => format!("Training runtime is not ready: {error}"),
            (_, Ok(_), Some(blocker)) => blocker.to_owned(),
            (_, Ok(backend), None) => format!(
                "Runtime ready · {probe} · Spiral project {} · namespace {} · query checked at run start",
                self.inner.storage.project_store().project_id(),
                spiral_robotics_storage::STUDIO_TABLE_PREFIX,
                probe = backend.display_summary(),
            ),
        };
        TrainingReadiness::new(ready, spiral_ready, summary).with_runtime(runtime)
    }

    /// The runtime endpoint Settings selects now: what probes check and new
    /// runs start on.
    async fn runtime_endpoint(&self) -> Result<String, String> {
        let preferences = self.inner.storage.preferences().load().await?;
        Ok(preferences.training().endpoint().to_owned())
    }

    async fn probe_backend(&self) -> Result<TrainingBackendInfo, String> {
        let endpoint = self.runtime_endpoint().await?;
        tokio::time::timeout(
            PROBE_TIMEOUT,
            HttpTrainingBackend::request(endpoint, |worker| worker.health()),
        )
        .await
        .map_err(|_| "training backend health probe timed out after 8 seconds".to_owned())?
        .and_then(|info| {
            match info.compatibility_error(spiral_robotics_storage::SCHEMA_VERSION) {
                Some(error) => Err(error),
                None => Ok(info),
            }
        })
    }
}

/// What each worker reports about the jobs it holds, asked once per endpoint.
struct WorkerJobs(BTreeMap<String, Result<BTreeMap<String, TrainingRunStatus>, String>>);

impl WorkerJobs {
    async fn fetch(records: &[TrainingRunRecord]) -> Self {
        let endpoints = records
            .iter()
            .map(|record| record.runtime_endpoint().to_owned())
            .collect::<BTreeSet<_>>();
        let reports = futures_util::future::join_all(endpoints.into_iter().map(|endpoint| async {
            let jobs = HttpTrainingBackend::request(endpoint.clone(), |worker| worker.jobs())
                .await
                .map(|jobs| {
                    jobs.into_iter()
                        .map(|job| (job.job_id().to_owned(), job.into_status()))
                        .collect()
                });
            (endpoint, jobs)
        }))
        .await;
        Self(reports.into_iter().collect())
    }

    /// The run `record` describes, with its worker's status or why there is none.
    fn run(&self, record: &TrainingRunRecord) -> TrainingRun {
        let endpoint = record.runtime_endpoint();
        let status = match &self.0[endpoint] {
            Ok(jobs) => jobs
                .get(record.identity().id())
                .cloned()
                .unwrap_or_else(|| {
                    TrainingRunStatus::unavailable(format!(
                        "The training runtime at {endpoint} no longer holds this run."
                    ))
                }),
            Err(error) => TrainingRunStatus::unavailable(format!(
                "The training runtime at {endpoint} cannot be reached: {error}"
            )),
        };
        TrainingRun::new(record.identity().clone(), status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::training::TrainingRunPhase;

    fn record(id: &str, endpoint: &str) -> TrainingRunRecord {
        TrainingRunRecord::new(
            TrainingRunIdentity::new(id, TrainingConfig::default(), 1),
            endpoint,
        )
    }

    #[test]
    fn runs_take_their_worker_status_or_say_why_there_is_none() {
        let running = TrainingRunStatus::new(TrainingRunPhase::Running, 7, None);
        let workers = WorkerJobs(BTreeMap::from([
            (
                "http://held".to_owned(),
                Ok(BTreeMap::from([("held".to_owned(), running)])),
            ),
            (
                "http://down".to_owned(),
                Err("connection refused".to_owned()),
            ),
        ]));

        let held = workers.run(&record("held", "http://held"));
        assert_eq!(held.phase(), TrainingRunPhase::Running);
        assert_eq!(held.current_step(), 7);

        let forgotten = workers.run(&record("forgotten", "http://held"));
        assert_eq!(forgotten.phase(), TrainingRunPhase::Unavailable);
        assert!(forgotten.message().unwrap().contains("no longer holds"));

        let unreachable = workers.run(&record("unreachable", "http://down"));
        assert_eq!(unreachable.phase(), TrainingRunPhase::Unavailable);
        assert!(
            unreachable
                .message()
                .unwrap()
                .contains("connection refused")
        );
        assert!(!unreachable.phase().active());
    }
}
