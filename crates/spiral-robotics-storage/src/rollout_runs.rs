//! Storage-owned relational row for the durable policy-rollout session.
//!
//! Studio keeps exactly one durable rollout session: the latest one, so a
//! restarted Studio can explain what the robot was last asked to do. That
//! single row is written here as typed columns owned by storage, never as a
//! serialized Studio contract, and this module is the only place that maps the
//! wire snapshot onto those columns field by field.

use std::sync::Arc;

use arrow_array::{
    ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_api::rollout::{
    RolloutIdentity, RolloutLifecycle, RolloutMetrics, RolloutPhase, RolloutSnapshot,
};

use crate::db::ArrowColumns;
use crate::db::column as db;
use crate::db::{ManagedRelationSchema, Table};
use crate::db::{ProjectStore, StorageClock};

/// Rollout relations contributed to project bundles.
pub(crate) struct RolloutBundleSection;

impl ManagedRelationSchema for RolloutBundleSection {
    fn relations(&self) -> &'static [Table] {
        &[Table::RolloutRuns]
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        (table == Table::RolloutRuns).then(|| RolloutRunSchema::session().as_ref().clone())
    }
}

/// Durable rollout-session operations for one project.
pub(crate) struct RolloutSessionPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> RolloutSessionPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn load(&self) -> Result<Option<RolloutSnapshot>, String> {
        if !self.project_store.is_configured() {
            return Ok(None);
        }
        let relation = Table::RolloutRuns.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect().await?;
        let sessions = connection
            .relation_rows(&relation, StoredRolloutSession::decode)
            .await?;
        Ok(sessions.last().map(StoredRolloutSession::restored))
    }

    pub(crate) async fn save(&self, snapshot: &RolloutSnapshot) -> Result<(), String> {
        let session = StoredRolloutSession::stored(snapshot);
        let batch = StoredRolloutSession::batch(session.as_ref(), StorageClock::unix_millis())?;
        let mut connection = self.project_store.connect().await?;
        connection
            .replace_batches(
                &Table::RolloutRuns.qualified_name(self.project_store.namespace()),
                vec![batch],
            )
            .await
    }
}

/// Lifecycle state recorded with a rollout session.
///
/// The persisted spelling is owned here; a renamed wire enumeration is mapped
/// explicitly below rather than silently changing stored bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoredRolloutPhase {
    Idle,
    Loading,
    WarmingUp,
    Running,
    Stopping,
    Failed,
}

impl StoredRolloutPhase {
    const fn stored(phase: RolloutPhase) -> Self {
        match phase {
            RolloutPhase::Idle => Self::Idle,
            RolloutPhase::Loading => Self::Loading,
            RolloutPhase::WarmingUp => Self::WarmingUp,
            RolloutPhase::Running => Self::Running,
            RolloutPhase::Stopping => Self::Stopping,
            RolloutPhase::Failed => Self::Failed,
        }
    }

    const fn restored(self) -> RolloutPhase {
        match self {
            Self::Idle => RolloutPhase::Idle,
            Self::Loading => RolloutPhase::Loading,
            Self::WarmingUp => RolloutPhase::WarmingUp,
            Self::Running => RolloutPhase::Running,
            Self::Stopping => RolloutPhase::Stopping,
            Self::Failed => RolloutPhase::Failed,
        }
    }

    const fn persisted(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::WarmingUp => "warming_up",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "idle" => Ok(Self::Idle),
            "loading" => Ok(Self::Loading),
            "warming_up" => Ok(Self::WarmingUp),
            "running" => Ok(Self::Running),
            "stopping" => Ok(Self::Stopping),
            "failed" => Ok(Self::Failed),
            other => Err(format!("stored rollout phase {other:?} is not known")),
        }
    }
}

/// What the session was asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredRolloutSelection {
    run_id: Option<String>,
    checkpoint_step: Option<u64>,
    robot: Option<String>,
    duration_seconds: Option<u32>,
}

/// How far the session got, and what it wants the operator to know.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredRolloutLifecycle {
    phase: StoredRolloutPhase,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    message: Option<String>,
    warning: Option<String>,
}

/// Counters accumulated while the policy held control.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredRolloutMetrics {
    inference_count: u64,
    command_count: u64,
    missed_deadlines: u64,
    last_latency_ms: Option<f64>,
    last_model_ms: Option<f64>,
    mean_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
}

/// The one durable rollout row, keyed by its session identity.
#[derive(Clone, Debug, PartialEq)]
struct StoredRolloutSession {
    session_id: String,
    selection: StoredRolloutSelection,
    lifecycle: StoredRolloutLifecycle,
    metrics: StoredRolloutMetrics,
}

impl StoredRolloutSession {
    /// A snapshot that never named a session has nothing durable to say, so it
    /// clears the relation instead of writing an identity-free row.
    fn stored(snapshot: &RolloutSnapshot) -> Option<Self> {
        let metrics = snapshot.metrics();
        Some(Self {
            session_id: snapshot.session_id()?.to_owned(),
            selection: StoredRolloutSelection {
                run_id: snapshot.run_id().map(str::to_owned),
                checkpoint_step: snapshot.checkpoint_step(),
                robot: snapshot.robot().map(str::to_owned),
                duration_seconds: snapshot.duration_seconds(),
            },
            lifecycle: StoredRolloutLifecycle {
                phase: StoredRolloutPhase::stored(snapshot.phase()),
                started_ms: snapshot.started_ms(),
                finished_ms: snapshot.finished_ms(),
                message: snapshot.message().map(str::to_owned),
                warning: snapshot.warning().map(str::to_owned),
            },
            metrics: StoredRolloutMetrics {
                inference_count: metrics.inference_count(),
                command_count: metrics.command_count(),
                missed_deadlines: metrics.missed_deadlines(),
                last_latency_ms: metrics.last_latency_ms(),
                last_model_ms: metrics.last_model_ms(),
                mean_latency_ms: metrics.mean_latency_ms(),
                max_latency_ms: metrics.max_latency_ms(),
            },
        })
    }

    fn restored(&self) -> RolloutSnapshot {
        RolloutSnapshot::new(
            RolloutIdentity::new(
                Some(self.session_id.clone()),
                self.selection.run_id.clone(),
                self.selection.robot.clone(),
            )
            .with_checkpoint(self.selection.checkpoint_step)
            .with_duration(self.selection.duration_seconds),
            RolloutLifecycle::new(self.lifecycle.phase.restored(), self.lifecycle.started_ms)
                .with_finished_ms(self.lifecycle.finished_ms)
                .with_messages(
                    self.lifecycle.message.clone(),
                    self.lifecycle.warning.clone(),
                ),
            RolloutMetrics::new(
                self.metrics.inference_count,
                self.metrics.command_count,
                self.metrics.missed_deadlines,
            )
            .with_latest_timings(self.metrics.last_latency_ms, self.metrics.last_model_ms)
            .with_latency_summary(self.metrics.mean_latency_ms, self.metrics.max_latency_ms),
        )
    }
}

/// Arrow schema for the managed `rollout_runs` relation.
struct RolloutRunSchema;

impl RolloutRunSchema {
    fn session() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::SESSION_ID, DataType::Utf8, false),
            Field::new(db::RUN_ID, DataType::Utf8, true),
            Field::new(db::CHECKPOINT_STEP, DataType::UInt64, true),
            Field::new(db::ROBOT, DataType::Utf8, true),
            Field::new(db::DURATION_SECONDS, DataType::UInt32, true),
            Field::new(db::PHASE, DataType::Utf8, false),
            Field::new(db::STARTED_MS, DataType::Int64, true),
            Field::new(db::FINISHED_MS, DataType::Int64, true),
            Field::new(db::MESSAGE, DataType::Utf8, true),
            Field::new(db::WARNING, DataType::Utf8, true),
            Field::new(db::INFERENCE_COUNT, DataType::UInt64, false),
            Field::new(db::COMMAND_COUNT, DataType::UInt64, false),
            Field::new(db::MISSED_DEADLINES, DataType::UInt64, false),
            Field::new(db::LAST_LATENCY_MS, DataType::Float64, true),
            Field::new(db::LAST_MODEL_MS, DataType::Float64, true),
            Field::new(db::MEAN_LATENCY_MS, DataType::Float64, true),
            Field::new(db::MAX_LATENCY_MS, DataType::Float64, true),
            Field::new(db::UPDATED_MS, DataType::Int64, false),
        ]))
    }
}

impl StoredRolloutSession {
    /// Encode the latest session as the relation's only row. A session-free
    /// state encodes zero rows, so an empty relation and a never-run project
    /// read back identically.
    fn batch(session: Option<&Self>, updated_ms: i64) -> Result<RecordBatch, String> {
        let rows = session.into_iter().collect::<Vec<_>>();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| &row.session_id),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.selection.run_id.as_deref()),
            )),
            Arc::new(UInt64Array::from_iter(
                rows.iter().map(|row| row.selection.checkpoint_step),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.selection.robot.as_deref()),
            )),
            Arc::new(UInt32Array::from_iter(
                rows.iter().map(|row| row.selection.duration_seconds),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|row| row.lifecycle.phase.persisted()),
            )),
            Arc::new(Int64Array::from_iter(
                rows.iter().map(|row| row.lifecycle.started_ms),
            )),
            Arc::new(Int64Array::from_iter(
                rows.iter().map(|row| row.lifecycle.finished_ms),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.lifecycle.message.as_deref()),
            )),
            Arc::new(StringArray::from_iter(
                rows.iter().map(|row| row.lifecycle.warning.as_deref()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.metrics.inference_count),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.metrics.command_count),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|row| row.metrics.missed_deadlines),
            )),
            Arc::new(Float64Array::from_iter(
                rows.iter().map(|row| row.metrics.last_latency_ms),
            )),
            Arc::new(Float64Array::from_iter(
                rows.iter().map(|row| row.metrics.last_model_ms),
            )),
            Arc::new(Float64Array::from_iter(
                rows.iter().map(|row| row.metrics.mean_latency_ms),
            )),
            Arc::new(Float64Array::from_iter(
                rows.iter().map(|row| row.metrics.max_latency_ms),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|_| updated_ms),
            )),
        ];
        RecordBatch::try_new(RolloutRunSchema::session(), columns)
            .map_err(|error| format!("building rollout session batch: {error}"))
    }

    fn decode(batch: &RecordBatch, row: usize) -> Result<Self, String> {
        let columns = ArrowColumns::new(batch, "rollout session");
        Ok(Self {
            session_id: columns.text(db::SESSION_ID, row)?.to_owned(),
            selection: StoredRolloutSelection {
                run_id: columns.optional_text(db::RUN_ID, row)?.map(str::to_owned),
                checkpoint_step: columns.optional_unsigned(db::CHECKPOINT_STEP, row)?,
                robot: columns.optional_text(db::ROBOT, row)?.map(str::to_owned),
                duration_seconds: columns
                    .optional_unsigned(db::DURATION_SECONDS, row)?
                    .map(|seconds| u32::try_from(seconds).unwrap_or(u32::MAX)),
            },
            lifecycle: StoredRolloutLifecycle {
                phase: StoredRolloutPhase::parse(columns.text(db::PHASE, row)?)?,
                started_ms: columns.optional_timestamp(db::STARTED_MS, row)?,
                finished_ms: columns.optional_timestamp(db::FINISHED_MS, row)?,
                message: columns.optional_text(db::MESSAGE, row)?.map(str::to_owned),
                warning: columns.optional_text(db::WARNING, row)?.map(str::to_owned),
            },
            metrics: StoredRolloutMetrics {
                inference_count: columns.unsigned(db::INFERENCE_COUNT, row)?,
                command_count: columns.unsigned(db::COMMAND_COUNT, row)?,
                missed_deadlines: columns.unsigned(db::MISSED_DEADLINES, row)?,
                last_latency_ms: columns.optional_float(db::LAST_LATENCY_MS, row)?,
                last_model_ms: columns.optional_float(db::LAST_MODEL_MS, row)?,
                mean_latency_ms: columns.optional_float(db::MEAN_LATENCY_MS, row)?,
                max_latency_ms: columns.optional_float(db::MAX_LATENCY_MS, row)?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> RolloutSnapshot {
        RolloutSnapshot::new(
            RolloutIdentity::new(
                Some("rollout-3".to_owned()),
                Some("train-7".to_owned()),
                Some("so101".to_owned()),
            )
            .with_checkpoint(Some(20_000))
            .with_duration(Some(45)),
            RolloutLifecycle::new(RolloutPhase::Failed, Some(1_700))
                .with_finished_ms(Some(1_900))
                .with_messages(Some("stopped".to_owned()), Some("check the arm".to_owned())),
            RolloutMetrics::new(9, 8, 1)
                .with_latest_timings(Some(4.5), Some(2.5))
                .with_latency_summary(Some(4.0), Some(9.0)),
        )
    }

    /// A fully populated wire session survives one typed row unchanged. A
    /// field added to the contract without a mapping here fails this test.
    #[test]
    fn a_populated_session_round_trips_through_one_typed_row() {
        let stored = StoredRolloutSession::stored(&snapshot()).expect("a named session is stored");
        assert_eq!(stored.session_id, "rollout-3");
        assert_eq!(stored.restored(), snapshot());

        let batch = StoredRolloutSession::batch(Some(&stored), 42).unwrap();
        assert_eq!(batch.num_rows(), 1);
        let decoded = StoredRolloutSession::decode(&batch, 0).unwrap();
        assert_eq!(decoded, stored);
        assert_eq!(decoded.restored(), snapshot());
    }

    #[test]
    fn a_session_free_state_encodes_no_rows() {
        assert!(StoredRolloutSession::stored(&RolloutSnapshot::default()).is_none());
        let batch = StoredRolloutSession::batch(None, 42).unwrap();
        assert_eq!(batch.num_rows(), 0);
    }

    #[test]
    fn unknown_persisted_phases_are_rejected_rather_than_guessed() {
        assert!(StoredRolloutPhase::parse("warming_up").is_ok());
        assert!(
            StoredRolloutPhase::parse("WarmingUp")
                .unwrap_err()
                .contains("not known")
        );
    }
}
