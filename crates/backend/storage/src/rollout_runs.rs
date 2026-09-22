//! Storage-owned JSON for every durable rollout session.

use spiral_robotics_common::rollout::{RolloutMetrics, RolloutPhase, RolloutSnapshot};

/// Durable rollout-session operations for one project.
pub(crate) struct RolloutSessionPersistence<'a> {
    project_store: &'a crate::StudioProject,
}

impl<'a> RolloutSessionPersistence<'a> {
    pub(crate) const fn new(project_store: &'a crate::StudioProject) -> Self {
        Self { project_store }
    }

    /// Every recorded session, in identity order.
    pub(crate) async fn load(&self) -> Result<Vec<RolloutSnapshot>, String> {
        use crate::studio::rollout_runs as table;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
        let rows = table::Entity::find()
            .filter(table::Column::Project.eq(self.project_store.key()))
            .order_by_asc(table::Column::Id)
            .all(self.project_store.connection().await?)
            .await
            .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                crate::studio::StoredDocument::decode::<StoredRolloutSession>(row.document)
                    .map(|session| session.restored())
            })
            .collect()
    }

    /// Write `snapshot` under its session. A snapshot that never named a
    /// session has nothing durable to say.
    pub(crate) async fn save(&self, snapshot: &RolloutSnapshot) -> Result<(), String> {
        use crate::studio::rollout_runs as table;
        use sea_orm::{EntityTrait, Set};
        let Some(session) = StoredRolloutSession::stored(snapshot) else {
            return Ok(());
        };
        table::Entity::insert(table::ActiveModel {
            project: Set(self.project_store.key().to_owned()),
            id: Set(session.session_id.clone()),
            document: Set(crate::studio::StoredDocument::encode(&session)?),
        })
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([table::Column::Project, table::Column::Id])
                .update_column(table::Column::Document)
                .to_owned(),
        )
        .exec(self.project_store.connection().await?)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// Lifecycle state recorded with a rollout session.
///
/// The persisted spelling is owned here; a renamed wire enumeration is mapped
/// explicitly below rather than silently changing stored bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
}

/// What the session was asked to run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredRolloutSelection {
    run_id: Option<String>,
    checkpoint_step: Option<u64>,
    robot: Option<String>,
    duration_seconds: Option<u32>,
}

/// How far the session got, and what it wants the operator to know.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredRolloutLifecycle {
    phase: StoredRolloutPhase,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    message: Option<String>,
    warning: Option<String>,
}

/// Counters accumulated while the policy held control.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredRolloutMetrics {
    inference_count: u64,
    command_count: u64,
    missed_deadlines: u64,
    #[serde(with = "crate::studio::float::optional")]
    last_latency_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    last_model_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    mean_latency_ms: Option<f64>,
    #[serde(with = "crate::studio::float::optional")]
    max_latency_ms: Option<f64>,
}

/// One durable rollout row, keyed by its session identity.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoredRolloutSession {
    session_id: String,
    selection: StoredRolloutSelection,
    lifecycle: StoredRolloutLifecycle,
    metrics: StoredRolloutMetrics,
}

impl StoredRolloutSession {
    /// A snapshot that never named a session has nothing durable to say.
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
        RolloutSnapshot::new(Some(self.session_id.clone()), self.selection.run_id.clone())
            .with_robot(self.selection.robot.clone())
            .with_checkpoint(self.selection.checkpoint_step)
            .with_duration(self.selection.duration_seconds)
            .with_lifecycle(self.lifecycle.phase.restored(), self.lifecycle.started_ms)
            .with_completion(self.lifecycle.finished_ms, self.lifecycle.message.clone())
            .with_warning(self.lifecycle.warning.clone())
            .with_metrics(
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

impl RolloutSessionPersistence<'_> {
    pub(crate) fn validate_record(id: &str, document: serde_json::Value) -> Result<(), String> {
        let session: StoredRolloutSession = crate::studio::StoredDocument::decode(document)?;
        if session.session_id.is_empty() || session.session_id != id {
            return Err("Rollout session identity disagrees with its row".into());
        }
        Ok(())
    }
}
