//! The studio's HTTP surface.
//!
//! Handlers translate delivery models into framework-neutral capability
//! commands and present their results. Capability use cases own admission and
//! orchestration; core entities and the serialized control thread provide the
//! final invariant and hardware-safety backstops.

mod data;
mod devices;
mod recordings;

use std::sync::Arc;
use std::time::Duration;

use crate::project_session::ProjectReadiness;
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::StreamExt;
use serde::Deserialize;
use spiral_robotics_common::status::StudioSnapshot;

use crate::data::catalog_lease::CatalogMutationLease;
use crate::data::queries::EpisodeQueries;
use crate::recording_session::RecordingSessionManager;
use crate::robotics::cameras::{CameraManager, Frame};
use crate::robotics::video::{VideoAsset, VideoCapture, VideoEncoderSettings};
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

        let cameras = state.cameras().statuses();
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
}

#[derive(Deserialize)]
struct ProjectQuery {
    session_id: String,
}

#[cfg(test)]
mod tests;
