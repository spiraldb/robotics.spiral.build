//! Storage-owned JSON documents for durable training history.

use spiral_robotics_common::training::{
    ActArchitectureConfig, ActAttentionConfig, ActInputConfig, ActLayerCounts, ActTrainingConfig,
    TrainingCheckpoint, TrainingConfig, TrainingMetricPoint, TrainingPolicy, TrainingRun,
    TrainingRunPhase, TrainingSnapshot, TrainingSource,
};

/// Durable training-history operations for one project.
pub(crate) struct TrainingHistoryPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> TrainingHistoryPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    pub(crate) async fn load(&self) -> Result<Vec<TrainingRun>, String> {
        use crate::studio::training_runs as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
        let rows = table::Entity::find()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_asc(table::Column::Ordinal)
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                crate::studio::StoredDocument::decode::<StoredTrainingRun>(row.document)
                    .map(|run| run.restored())
            })
            .collect()
    }

    pub(crate) async fn save(&self, snapshot: &TrainingSnapshot) -> Result<(), String> {
        use crate::studio::training_runs as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
        let transaction = self
            .project_store
            .connection()
            .await?
            .begin()
            .await
            .map_err(|error| error.to_string())?;
        let ids = snapshot
            .runs()
            .iter()
            .map(|run| run.id().to_owned())
            .collect::<Vec<_>>();
        let retained = table::Entity::delete_many()
            .filter(table::Column::Project.eq(self.project_store.key()));
        if ids.is_empty() {
            retained
                .exec(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        } else {
            retained
                .filter(table::Column::Id.is_not_in(ids))
                .exec(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        for (ordinal, run) in snapshot.runs().iter().enumerate() {
            let model = table::ActiveModel {
                project: Set(self.project_store.key().to_owned()),
                id: Set(run.id().to_owned()),
                ordinal: Set(i64::try_from(ordinal).map_err(|_| "Too many training runs")?),
                document: Set(crate::studio::StoredDocument::encode(
                    &StoredTrainingRun::stored(run),
                )?),
            };
            table::Entity::insert(model)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        table::Column::Project,
                        table::Column::Id,
                    ])
                    .update_columns([table::Column::Ordinal, table::Column::Document])
                    .to_owned(),
                )
                .exec(&transaction)
                .await
                .map_err(|error| error.to_string())?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())
    }
}

/// Policy family recorded with a training run.
///
/// The persisted spelling is owned here; a renamed wire enumeration is mapped
/// explicitly below rather than silently changing stored bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
enum StoredTrainingPolicy {
    Act,
}

impl StoredTrainingPolicy {
    const fn stored(policy: TrainingPolicy) -> Self {
        match policy {
            TrainingPolicy::Act => Self::Act,
        }
    }
}

/// Lifecycle state recorded with a training run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
enum StoredTrainingPhase {
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl StoredTrainingPhase {
    const fn stored(phase: TrainingRunPhase) -> Self {
        match phase {
            TrainingRunPhase::Starting => Self::Starting,
            TrainingRunPhase::Running => Self::Running,
            TrainingRunPhase::Stopping => Self::Stopping,
            TrainingRunPhase::Completed => Self::Completed,
            TrainingRunPhase::Stopped => Self::Stopped,
            TrainingRunPhase::Failed => Self::Failed,
        }
    }

    const fn restored(self) -> TrainingRunPhase {
        match self {
            Self::Starting => TrainingRunPhase::Starting,
            Self::Running => TrainingRunPhase::Running,
            Self::Stopping => TrainingRunPhase::Stopping,
            Self::Completed => TrainingRunPhase::Completed,
            Self::Stopped => TrainingRunPhase::Stopped,
            Self::Failed => TrainingRunPhase::Failed,
        }
    }
}

/// Immutable identity of one persisted run.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingIdentity {
    run_id: String,
    run_name: String,
    /// Dataset the run actually drew episodes from, captured at start.
    source_dataset_id: String,
}

/// Operator-chosen training set and execution target.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingSelection {
    dataset_id: String,
    policy: StoredTrainingPolicy,
    query_sql: String,
    state_column: String,
    action_column: String,
    device: String,
}

/// Optimizer schedule recorded with a run.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingOptimization {
    steps: u64,
    batch_size: u64,
    #[serde(with = "crate::studio::float")]
    validation_split: f64,
    #[serde(with = "crate::studio::float")]
    learning_rate: f64,
    checkpoint_every: u64,
    chunk_size: u64,
    seed: u64,
}

/// Sample-preparation settings recorded with an ACT run.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredActInput {
    camera_streams: Vec<String>,
    image_width: u64,
    image_height: u64,
    video_decoder: String,
    pretrained_backbone: bool,
}

/// Transformer depth recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredActLayerCounts {
    encoder: u64,
    variational_encoder: u64,
    decoder: u64,
}

/// Model shape recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredActArchitecture {
    hidden_dim: u64,
    latent_dim: u64,
    layers: StoredActLayerCounts,
}

/// Attention settings recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredActAttention {
    attention_heads: u64,
    feedforward_dim: u64,
    #[serde(with = "crate::studio::float")]
    kl_weight: f64,
}

/// Complete ACT configuration recorded with a run.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredActConfig {
    input: StoredActInput,
    architecture: StoredActArchitecture,
    attention: StoredActAttention,
}

impl StoredActConfig {
    fn stored(act: &ActTrainingConfig) -> Self {
        Self {
            input: StoredActInput {
                camera_streams: act.camera_streams().to_vec(),
                image_width: act.image_width() as u64,
                image_height: act.image_height() as u64,
                video_decoder: act.video_decoder().to_owned(),
                pretrained_backbone: act.pretrained_backbone(),
            },
            architecture: StoredActArchitecture {
                hidden_dim: act.hidden_dim() as u64,
                latent_dim: act.latent_dim() as u64,
                layers: StoredActLayerCounts {
                    encoder: act.encoder_layers() as u64,
                    variational_encoder: act.vae_encoder_layers() as u64,
                    decoder: act.decoder_layers() as u64,
                },
            },
            attention: StoredActAttention {
                attention_heads: act.attention_heads() as u64,
                feedforward_dim: act.feedforward_dim() as u64,
                kl_weight: act.kl_weight(),
            },
        }
    }

    fn restored(&self) -> ActTrainingConfig {
        ActTrainingConfig::new(
            ActInputConfig::new(
                self.input.camera_streams.clone(),
                (
                    self.input.image_width as usize,
                    self.input.image_height as usize,
                ),
                self.input.video_decoder.clone(),
            )
            .with_pretrained_backbone(self.input.pretrained_backbone),
            ActArchitectureConfig::new(
                self.architecture.hidden_dim as usize,
                self.architecture.latent_dim as usize,
                ActLayerCounts::new(
                    self.architecture.layers.encoder as usize,
                    self.architecture.layers.variational_encoder as usize,
                    self.architecture.layers.decoder as usize,
                ),
            ),
            ActAttentionConfig::new(
                self.attention.attention_heads as usize,
                self.attention.feedforward_dim as usize,
                self.attention.kl_weight,
            ),
        )
    }
}

/// Mutable execution state recorded with a run.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingLifecycle {
    phase: StoredTrainingPhase,
    current_step: u64,
    started_ms: i64,
    finished_ms: Option<i64>,
    message: Option<String>,
}

/// One sample on a run's optimizer-step axis.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingProgress {
    step: u64,
    #[serde(with = "crate::studio::float")]
    train_loss: f64,
    #[serde(with = "crate::studio::float::optional")]
    validation_loss: Option<f64>,
    #[serde(with = "crate::studio::float")]
    learning_rate: f64,
    #[serde(with = "crate::studio::float")]
    elapsed_seconds: f64,
    event: Option<String>,
}

/// Host and device utilization sampled with one metric point.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingResources {
    #[serde(with = "crate::studio::float")]
    transfer_bytes_per_second: f64,
    #[serde(with = "crate::studio::float")]
    examples_per_second: f64,
    #[serde(with = "crate::studio::float::optional")]
    cpu_percent: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    gpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
}

/// Per-stage durations sampled with one metric point.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingTimings {
    #[serde(with = "crate::studio::float::optional")]
    loader_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    arrow_encode_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    upload_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    device_copy_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    model_ms: Option<f64>,
}

/// One metric point together with the utilization sampled alongside it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingMetric {
    progress: StoredTrainingProgress,
    resources: StoredTrainingResources,
    timings: StoredTrainingTimings,
}

impl StoredTrainingMetric {
    /// Floats cross verbatim in both directions: Arrow holds a diverged run's
    /// NaN natively, so no measurement is invented or discarded here.
    fn stored(metric: &TrainingMetricPoint) -> Self {
        Self {
            progress: StoredTrainingProgress {
                step: metric.step(),
                train_loss: metric.train_loss(),
                validation_loss: metric.validation_loss(),
                learning_rate: metric.learning_rate(),
                elapsed_seconds: metric.elapsed_seconds(),
                event: metric.event().map(str::to_owned),
            },
            resources: StoredTrainingResources {
                transfer_bytes_per_second: metric.transfer_bytes_per_second(),
                examples_per_second: metric.examples_per_second(),
                cpu_percent: metric.cpu_percent(),
                gpu_percent: metric.gpu_percent(),
                memory_bytes: metric.memory_bytes(),
            },
            timings: StoredTrainingTimings {
                loader_ms: metric.loader_ms(),
                arrow_encode_ms: metric.arrow_encode_ms(),
                upload_ms: metric.upload_ms(),
                device_copy_ms: metric.device_copy_ms(),
                model_ms: metric.model_ms(),
            },
        }
    }

    fn restored(&self) -> TrainingMetricPoint {
        TrainingMetricPoint::new(self.progress.step, self.progress.train_loss)
            .with_learning_rate(self.progress.learning_rate)
            .with_validation_loss(self.progress.validation_loss)
            .with_timeline(self.progress.elapsed_seconds, self.progress.event.clone())
            .with_throughput(
                self.resources.transfer_bytes_per_second,
                self.resources.examples_per_second,
            )
            .with_utilization(self.resources.cpu_percent, self.resources.gpu_percent)
            .with_memory_bytes(self.resources.memory_bytes)
            .with_loader_ms(self.timings.loader_ms)
            .with_transport_timings(self.timings.arrow_encode_ms, self.timings.upload_ms)
            .with_runtime_timings(self.timings.device_copy_ms, self.timings.model_ms)
    }
}

/// One inference checkpoint published by a run.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingCheckpoint {
    step: u64,
    created_ms: i64,
    #[serde(with = "crate::studio::float::optional")]
    validation_loss: Option<f64>,
    byte_size: u64,
    inference_ready: bool,
    recommended_action_steps: u64,
    #[serde(with = "crate::studio::float")]
    sample_hz: f64,
}

impl StoredTrainingCheckpoint {
    fn stored(checkpoint: &TrainingCheckpoint) -> Self {
        Self {
            step: checkpoint.step(),
            created_ms: checkpoint.created_ms(),
            validation_loss: checkpoint.validation_loss(),
            byte_size: checkpoint.byte_size(),
            inference_ready: checkpoint.inference_ready(),
            recommended_action_steps: checkpoint.recommended_action_steps() as u64,
            sample_hz: checkpoint.sample_hz(),
        }
    }

    fn restored(&self) -> TrainingCheckpoint {
        TrainingCheckpoint::new(self.step, self.created_ms)
            .with_validation_loss(self.validation_loss)
            .with_artifact(self.byte_size, self.inference_ready)
            .with_rollout(self.recommended_action_steps as usize, self.sample_hz)
    }
}

/// The child rows keyed by a run's `run_id`, plus its bounded log tail.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingRunHistory {
    metrics: Vec<StoredTrainingMetric>,
    checkpoints: Vec<StoredTrainingCheckpoint>,
    logs: Vec<String>,
}

/// One durable training run and the child rows keyed by its `run_id`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredTrainingRun {
    identity: StoredTrainingIdentity,
    selection: StoredTrainingSelection,
    optimization: StoredTrainingOptimization,
    act: StoredActConfig,
    lifecycle: StoredTrainingLifecycle,
    history: StoredTrainingRunHistory,
}

impl StoredTrainingRun {
    fn stored(run: &TrainingRun) -> Self {
        let config = run.config();
        Self {
            identity: StoredTrainingIdentity {
                run_id: run.id().to_owned(),
                run_name: config.run_name().to_owned(),
                source_dataset_id: run.source().dataset_id().to_owned(),
            },
            selection: StoredTrainingSelection {
                dataset_id: config.dataset_id().to_owned(),
                policy: StoredTrainingPolicy::stored(config.policy()),
                query_sql: config.sql().to_owned(),
                state_column: config.state_column().to_owned(),
                action_column: config.action_column().to_owned(),
                device: config.device().to_owned(),
            },
            optimization: StoredTrainingOptimization {
                steps: config.steps(),
                batch_size: config.batch_size() as u64,
                validation_split: config.validation_split(),
                learning_rate: config.learning_rate(),
                checkpoint_every: config.checkpoint_every(),
                chunk_size: config.chunk_size() as u64,
                seed: config.seed(),
            },
            act: StoredActConfig::stored(config.act()),
            lifecycle: StoredTrainingLifecycle {
                phase: StoredTrainingPhase::stored(run.phase()),
                current_step: run.current_step(),
                started_ms: run.started_ms(),
                finished_ms: run.finished_ms(),
                message: run.message().map(str::to_owned),
            },
            history: StoredTrainingRunHistory {
                metrics: run
                    .metrics()
                    .iter()
                    .map(StoredTrainingMetric::stored)
                    .collect(),
                checkpoints: run
                    .checkpoints()
                    .iter()
                    .map(StoredTrainingCheckpoint::stored)
                    .collect(),
                logs: run.logs().to_vec(),
            },
        }
    }

    fn restored(&self) -> TrainingRun {
        TrainingRun::starting(&self.identity.run_id, self.restored_config())
            .with_source(TrainingSource::new(&self.identity.source_dataset_id))
            .with_phase(self.lifecycle.phase.restored(), self.lifecycle.current_step)
            .with_started_ms(self.lifecycle.started_ms)
            .with_completion(self.lifecycle.finished_ms, self.lifecycle.message.clone())
            .with_metrics(
                self.history
                    .metrics
                    .iter()
                    .map(StoredTrainingMetric::restored)
                    .collect(),
            )
            .with_checkpoints(
                self.history
                    .checkpoints
                    .iter()
                    .map(StoredTrainingCheckpoint::restored)
                    .collect(),
            )
            .with_logs(self.history.logs.clone())
    }

    fn restored_config(&self) -> TrainingConfig {
        // Both policy enums have the single ACT variant today; the match keeps
        // a future storage variant from silently restoring as the default.
        match self.selection.policy {
            StoredTrainingPolicy::Act => {}
        }
        let mut config = TrainingConfig::for_dataset(
            &self.identity.run_name,
            &self.selection.dataset_id,
            &self.selection.query_sql,
        );
        config.set_device(&self.selection.device);
        config.set_state_column(&self.selection.state_column);
        config.set_action_column(&self.selection.action_column);
        config.set_steps(self.optimization.steps);
        config.set_batch_size(self.optimization.batch_size as usize);
        config.set_validation_split(self.optimization.validation_split);
        config.set_learning_rate(self.optimization.learning_rate);
        config.set_checkpoint_every(self.optimization.checkpoint_every);
        config.set_chunk_size(self.optimization.chunk_size as usize);
        config.set_seed(self.optimization.seed);
        *config.act_mut() = self.act.restored();
        config
    }
}

impl TrainingHistoryPersistence<'_> {
    pub(crate) fn validate_record(id: &str, document: serde_json::Value) -> Result<(), String> {
        let run: StoredTrainingRun = crate::studio::StoredDocument::decode(document)?;
        if run.identity.run_id != id || id.is_empty() {
            return Err("Training document identity disagrees with its row".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use sea_orm::ConnectionTrait;
    use spiral_robotics_common::training::TrainingReadiness;

    #[tokio::test]
    async fn history_preserves_order_nonfinite_metrics_and_rolls_back_failed_saves() {
        let store = crate::StudioStore::in_memory();
        let connection = crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("training")
        .unwrap();
        let project = store.project(&connection);
        let persistence = TrainingHistoryPersistence::new(&project);
        let run = TrainingRun::starting("diverged", TrainingConfig::default())
            .with_metrics(vec![TrainingMetricPoint::new(1, f64::NAN)]);
        let snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![
                run,
                TrainingRun::starting("second", TrainingConfig::default()),
            ],
        );
        persistence.save(&snapshot).await.unwrap();
        let restored = persistence.load().await.unwrap();
        assert_eq!(
            restored.iter().map(TrainingRun::id).collect::<Vec<_>>(),
            ["diverged", "second"]
        );
        assert!(restored[0].metrics()[0].train_loss().is_nan());
        // A failure after deletion must restore both runs and their histories.
        let database = store.connection().await.unwrap();
        database.execute_unprepared("CREATE TRIGGER reject_history BEFORE UPDATE ON training_runs BEGIN SELECT RAISE(ABORT, 'test failure'); END;").await.unwrap();
        let reduced =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![restored[0].clone()]);
        assert!(persistence.save(&reduced).await.is_err());
        assert_eq!(persistence.load().await.unwrap().len(), 2);
        database
            .execute_unprepared("DROP TRIGGER reject_history")
            .await
            .unwrap();
        persistence.save(&reduced).await.unwrap();
        assert_eq!(persistence.load().await.unwrap().len(), 1);
        persistence
            .save(&TrainingSnapshot::default())
            .await
            .unwrap();
        assert!(persistence.load().await.unwrap().is_empty());
    }
}
