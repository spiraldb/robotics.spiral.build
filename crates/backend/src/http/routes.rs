//! The studio's HTTP surface.
//!
//! Handlers translate delivery models into framework-neutral capability
//! commands and present their results. Capability use cases own admission and
//! orchestration; core entities and the serialized control thread provide the
//! final invariant and hardware-safety backstops.

mod data;
mod devices;
mod recordings;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use crate::project_session::ProjectReadiness;
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use spiral_robotics_common::status::StudioSnapshot;

use crate::data::catalog_lease::CatalogMutationLease;
use crate::data::queries::EpisodeQueries;
use crate::recording_session::RecordingSessionManager;
use crate::robotics::cameras::{CameraManager, Frame};
use crate::robotics::video::{VideoAsset, VideoCapture, VideoEncoderSettings};
use crate::robots::discovery::CameraDiscoveryPort;
use crate::robots::service::RobotCatalogActions;
use crate::robots::workspace::RobotWorkspace;
use crate::rollout::manager::RolloutManager;
use crate::training::manager::TrainingManager;
use crate::workflow::agent::WorkflowAgentManager;
use crate::workflow::manager::WorkflowManager;
use spiral_robotics_hardware::HardwareHandle;
use spiral_robotics_storage::StorageManager;
use spiral_robotics_storage::StudioPreferencesStore;
use spiral_robotics_storage::episodes::EpisodeStore;
use spiral_robotics_storage::video::{VideoAssetKey, VideoByteRange};

/// Boundary marker separating parts of an MJPEG response.
const MJPEG_BOUNDARY: &str = "spiral-robotics-frame";

/// How often live telemetry is pushed to the browser.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(50);

/// How often the setup-level snapshot is rebuilt and diffed.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);

/// A short debounce keeps a just-started recording from being mistaken for an
/// automatic stop before the next telemetry snapshot is published.
const VIDEO_RECORDING_MONITOR_INTERVAL: Duration = Duration::from_millis(200);
const VIDEO_RECORDING_STOP_TICKS: u8 = 5;

/// A request or task retains one project for its entire lifetime.
#[derive(Clone)]
pub(crate) struct AppState {
    project: Arc<crate::project_session::ProjectSession>,
    projects: Arc<crate::project_session::ProjectCoordinator>,
}

impl AppState {
    pub(crate) fn new(projects: Arc<crate::project_session::ProjectCoordinator>) -> Self {
        Self {
            project: projects.active(),
            projects,
        }
    }

    pub(crate) fn current(&self) -> Self {
        Self::new(self.projects.clone())
    }

    pub(crate) fn for_session(&self, session_id: &str) -> Result<Self, crate::OperationError> {
        let project = self.projects.resolve(session_id)?;
        Ok(Self {
            project,
            projects: self.projects.clone(),
        })
    }

    pub(crate) fn admit(
        &self,
    ) -> Result<crate::project_admission::ProjectActivity, crate::OperationError> {
        self.projects.admit(self.project.info().id())
    }

    pub(crate) fn project(&self) -> &Arc<crate::project_session::ProjectSession> {
        &self.project
    }
    pub(crate) fn projects(&self) -> &Arc<crate::project_session::ProjectCoordinator> {
        &self.projects
    }
    pub(crate) fn workspace(&self) -> &Arc<RobotWorkspace> {
        self.project.workspace()
    }
    pub(crate) fn episodes(&self) -> &Arc<EpisodeStore> {
        self.project.episodes()
    }
    pub(crate) fn storage(&self) -> &Arc<StorageManager> {
        self.project.storage()
    }
    pub(crate) fn catalog_lease(&self) -> &Arc<CatalogMutationLease> {
        self.project.catalog_lease()
    }
    pub(crate) fn hardware(&self) -> &Arc<HardwareHandle> {
        self.projects.hardware()
    }
    pub(crate) fn cameras(&self) -> &Arc<CameraManager> {
        self.projects.cameras()
    }
    pub(crate) fn recording_session(&self) -> &Arc<RecordingSessionManager> {
        self.project.recording()
    }
    pub(crate) fn video_encoder(&self) -> &VideoEncoderSettings {
        self.projects.video_encoder()
    }
    pub(crate) fn training(&self) -> &Arc<TrainingManager> {
        self.project.training()
    }
    pub(crate) fn workflow(&self) -> &Arc<WorkflowManager> {
        self.project.workflow()
    }
    pub(crate) fn workflow_agent(&self) -> &Arc<WorkflowAgentManager> {
        self.project.workflow_agent()
    }
    pub(crate) fn rollout(&self) -> &Arc<RolloutManager> {
        self.project.rollout()
    }
    pub(crate) fn preferences(&self) -> &Arc<StudioPreferencesStore> {
        self.project.preferences()
    }
    pub(crate) fn bootstrap(&self) -> &ProjectReadiness {
        self.project.readiness()
    }
}

impl AppState {
    pub(crate) async fn monitor_video_recording(self) {
        let mut ticker = tokio::time::interval(VIDEO_RECORDING_MONITOR_INTERVAL);
        let mut missing_episode = String::new();
        let mut missing_ticks = 0_u8;
        loop {
            ticker.tick().await;
            let state = self.current();
            let Some(capture_episode) = state.recording_session().episode() else {
                missing_episode.clear();
                missing_ticks = 0;
                continue;
            };
            let hardware = self.hardware().state();
            let hardware_episode = hardware
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned());
            if hardware_episode.as_deref() == Some(&capture_episode) {
                missing_episode.clear();
                missing_ticks = 0;
                continue;
            }
            if missing_episode == capture_episode {
                missing_ticks = missing_ticks.saturating_add(1);
            } else {
                missing_episode = capture_episode;
                missing_ticks = 1;
            }
            if missing_ticks < VIDEO_RECORDING_STOP_TICKS {
                continue;
            }
            state.finish_stale_video_capture().await;
            missing_episode.clear();
            missing_ticks = 0;
        }
    }

    /// Close and commit any capture whose corresponding control-loop recording
    /// has already ended. Also used during graceful backend shutdown.
    pub(crate) async fn finish_stale_video_capture(&self) {
        // Claim the recording exactly once, but retain catalog exclusion until the
        // episode and finalized media have both been committed. This exists
        // independently of VideoCapture because camera-less recordings still
        // allocate catalog identities.
        let token = match self.recording_session().claim_finalization() {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!("could not claim stale recording finalization: {error}");
                return;
            }
        };
        let capture = self.recording_session().take_media().await;
        if token.is_none() && capture.is_none() {
            return;
        }
        let episode_id = self.recording_session().episode().or_else(|| {
            capture
                .as_ref()
                .map(|capture| capture.episode_id().to_owned())
        });
        let videos = match capture {
            Some(capture) => self.finish_video_capture(capture).await,
            None => Vec::new(),
        };
        if let Some(episode_id) = episode_id {
            let committed = async {
                let episode = self
                    .episodes()
                    .completed_episode(&episode_id)
                    .map_err(|error| error.to_string())?;
                self.storage().commit_episode(&episode, &videos).await?;
                self.episodes().forget_completed(&episode_id);
                Ok::<_, String>(())
            }
            .await;
            if let Err(error) = committed {
                tracing::error!("could not commit automatically stopped episode media: {error}");
                self.cameras().note_issue(format!(
                    "episode media could not be committed to SpiralDB: {error}"
                ));
            }
        }
        if let Some(token) = token {
            self.recording_session().release(token);
        }
    }

    pub(crate) async fn finish_video_capture(&self, capture: VideoCapture) -> Vec<VideoAsset> {
        let (videos, failures) = capture.finish().await;
        for failure in failures {
            self.cameras()
                .note_issue(format!("episode video was not saved: {failure}"));
        }
        videos
    }
}

/// Build the studio's API routes.
impl AppState {
    pub(crate) fn api_router(self) -> Router {
        Router::new()
            .merge(Self::data_transfer_router())
            .merge(Self::device_bootstrap_router())
            .merge(Self::recording_storage_router())
            .route("/events", get(Self::events))
            .route("/session", get(Self::session_info))
            .with_state(self)
    }

    /// Assemble the current setup-level view of the studio.
    pub(crate) async fn snapshot(&self) -> StudioSnapshot {
        let state = self;
        let hardware = state.hardware().state();
        let readiness = state.bootstrap().current();
        let robots = if readiness.catalog_available() {
            let robot_adapter = crate::device_commands::RobotAdapter::prefetched(state).await;
            let mut summaries = Vec::new();
            for saved in RobotCatalogActions::new(&robot_adapter).list().await {
                let connection = hardware
                    .connections()
                    .get(saved.id())
                    .cloned()
                    .unwrap_or_default();
                let summary = robot_adapter.summary(saved.id(), connection).await.ok();
                let Some(mut summary) = summary else {
                    continue;
                };
                // Limits come from the motors, not the saved configuration, so they
                // are folded in here after loading the SpiralDB row.
                if let Some(limits) = hardware.limits().get(saved.id()) {
                    let motors = summary
                        .motors()
                        .iter()
                        .cloned()
                        .map(|mut motor| {
                            if let Some(limits) = limits.get(motor.name()) {
                                motor.set_limits(*limits);
                            }
                            motor
                        })
                        .collect();
                    summary = summary.with_motors(motors);
                }
                summaries.push(summary);
            }
            summaries
        } else {
            Vec::new()
        };

        let mut issues = if readiness.catalog_available() {
            state.workspace().issues()
        } else {
            Vec::new()
        };
        issues.extend(state.cameras().issues());
        // A take that stopped itself has to be visible after the counters that were
        // describing it have gone.
        if let Some(issue) = hardware.recording_issue() {
            issues.push(issue.to_owned());
        }

        let cameras =
            crate::device_commands::CameraDiscoveryAdapter::new(state.cameras().clone()).cameras();
        StudioSnapshot::new(readiness, "SpiralDB", hardware.mode_status().clone())
            .with_robots(robots)
            .with_cameras(cameras)
            .with_issues(issues)
    }

    async fn session_info(
        State(state): State<Self>,
    ) -> axum::Json<spiral_robotics_common::project::ProjectSessionInfo> {
        axum::Json(state.projects().active().info().clone())
    }

    /// Each stream generation owns exactly one session. A replacement first
    /// announces its identity, then sends fresh snapshots and rebinds receivers.
    async fn events(
        State(state): State<Self>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let stream = async_stream::stream! {
            let mut sessions = state.projects().subscribe();
            loop {
                let project = sessions.borrow_and_update().clone();
                let scoped = AppState { project, projects: state.projects().clone() };
                let wire = ProjectEventStream::new(scoped.project().info().id());
                let (initial_training, mut training) = scoped.training().subscription();
                let (initial_workflow, mut workflow) = scoped.workflow().subscription();
                let (initial_agent, mut workflow_agent) = scoped.workflow_agent().subscription();
                let mut rollout = scoped.rollout().subscribe();
                let mut readiness = scoped.bootstrap().subscribe();
                let mut snapshot_timer = tokio::time::interval(SNAPSHOT_INTERVAL);
                snapshot_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut telemetry_timer = tokio::time::interval(TELEMETRY_INTERVAL);
                let mut last_snapshot: Option<StudioSnapshot> = None;
                let mut last_sequence = 0_u64;
                let mut last_agent_cursor = AgentCursor::from(&initial_agent);
                if let Ok(event) = Event::default().event("project_session").json_data(scoped.project().info()) { yield Ok(event); }
                for event in [
                    wire.encode("training_snapshot", &initial_training),
                    wire.encode("rollout_state", &scoped.rollout().snapshot()),
                    wire.encode("workflow_snapshot", &initial_workflow),
                    wire.encode("workflow_agent", &initial_agent),
                ].into_iter().flatten() { yield Ok(event); }
                loop {
                    tokio::select! {
                        biased;
                        changed = sessions.changed() => {
                            if changed.is_err() { return; }
                            break;
                        }
                        changed = readiness.changed() => {
                            if changed.is_err() { break; }
                            let snapshot = tokio::select! {
                                biased;
                                _ = sessions.changed() => break,
                                changed = readiness.changed() => {
                                    if changed.is_err() { break; }
                                    readiness.mark_changed();
                                    continue;
                                },
                                snapshot = scoped.snapshot() => snapshot,
                            };
                            if let Some(event) = wire.encode("state", &snapshot) { yield Ok(event); }
                            last_snapshot = Some(snapshot);
                        }
                        _ = snapshot_timer.tick() => {
                            let snapshot = tokio::select! {
                                biased;
                                _ = sessions.changed() => break,
                                // Never emit a ready snapshot assembled before
                                // a project change began. Rebuild it immediately
                                // without waiting for catalog I/O to complete.
                                changed = readiness.changed() => {
                                    if changed.is_err() { break; }
                                    readiness.mark_changed();
                                    continue;
                                },
                                snapshot = scoped.snapshot() => snapshot,
                            };
                            if last_snapshot.as_ref() != Some(&snapshot) {
                                if let Some(event) = wire.encode("state", &snapshot) { yield Ok(event); }
                                last_snapshot = Some(snapshot);
                            }
                        }
                        _ = telemetry_timer.tick() => {
                            let telemetry = scoped.hardware().state().telemetry().clone();
                            if telemetry.sequence() != last_sequence {
                                last_sequence = telemetry.sequence();
                                if let Some(event) = wire.encode("telemetry", &telemetry) { yield Ok(event); }
                            }
                        }
                        update = training.recv() => {
                            match update {
                                Ok(update) => if let Some(event) = wire.encode("training_update", &update) { yield Ok(event); },
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    let (snapshot, replacement) = scoped.training().subscription();
                                    training = replacement;
                                    if let Some(event) = wire.encode("training_snapshot", &snapshot) { yield Ok(event); }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                        update = workflow.recv() => {
                            if matches!(update, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; }
                            if let Some(event) = wire.encode("workflow_snapshot", &scoped.workflow().state()) { yield Ok(event); }
                        }
                        update = workflow_agent.recv() => {
                            if matches!(update, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; }
                            let snapshot = scoped.workflow_agent().state();
                            if last_agent_cursor.advance_to(AgentCursor::from(&snapshot))
                                && let Some(event) = wire.encode("workflow_agent", &snapshot) {
                                yield Ok(event);
                            }
                        }
                        changed = rollout.changed() => {
                            if changed.is_err() { break; }
                            let snapshot = rollout.borrow_and_update().clone();
                            if let Some(event) = wire.encode("rollout_state", &snapshot) { yield Ok(event); }
                        }
                    }
                }
            }
        };
        Sse::new(stream).keep_alive(KeepAlive::default())
    }
}

struct ProjectEventStream {
    session_id: String,
}

impl ProjectEventStream {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
        }
    }

    fn encode<T: serde::Serialize>(&self, name: &str, payload: &T) -> Option<Event> {
        Event::default()
            .event(name)
            .json_data(spiral_robotics_common::project::ProjectEvent::new(
                &self.session_id,
                payload,
            ))
            .ok()
    }
}

#[derive(Deserialize)]
struct ProjectQuery {
    session_id: String,
}

struct AgentCursor {
    scope_id: String,
    revision: u64,
}

impl AgentCursor {
    fn advance_to(&mut self, next: Self) -> bool {
        if self.scope_id == next.scope_id && self.revision == next.revision {
            return false;
        }
        *self = next;
        true
    }
}

impl From<&spiral_robotics_common::workflow::WorkflowAgentSnapshot> for AgentCursor {
    fn from(state: &spiral_robotics_common::workflow::WorkflowAgentSnapshot) -> Self {
        Self {
            scope_id: state.scope_id().to_owned(),
            revision: state.revision(),
        }
    }
}

#[cfg(test)]
mod tests;
