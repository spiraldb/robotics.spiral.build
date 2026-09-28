//! HTTP endpoints for episodes.
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingCameraRequest, RecordingFinish,
    RecordingStart,
};
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use super::catalog::EpisodeCatalog;
use super::take::TakeOperations;
use crate::OperationError;
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, Created, Project};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post},
};

pub(crate) struct RecordingEndpoints;

impl RecordingEndpoints {
    pub(crate) fn reads() -> Router<StudioContext> {
        Router::new()
            .route("/recording/signals", get(Self::fetch_recording_signals))
            .route("/recording/takes/{id}", get(Self::inspect_episode))
            .route("/recording/takes/{id}/camera", post(Self::camera_preview))
    }
    pub(crate) fn commands() -> Router<StudioContext> {
        Router::new()
            .route(
                "/recording",
                post(Self::start_recording).patch(Self::set_recording_paused),
            )
            .route("/recording/stop", post(Self::stop_recording))
    }
    async fn fetch_recording_signals(
        Project(project): Project,
        ApiQuery(cursor): ApiQuery<RecordingCursor>,
    ) -> Json<RecordingWindow> {
        Json(project.hardware().recording_signals(cursor))
    }
    async fn camera_preview(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<RecordingCameraRequest>,
    ) -> Result<Json<EpisodeVideo>, OperationError> {
        Ok(Json(
            EpisodeCatalog::new(&project).camera_preview(id, request)?,
        ))
    }
    async fn start_recording(
        Project(project): Project,
        ApiJson(request): ApiJson<RecordingStart>,
    ) -> Result<Created<EpisodeInfo>, OperationError> {
        let summary = TakeOperations::new(&project).start(request.into()).await?;
        Ok(Created(summary))
    }
    async fn stop_recording(
        Project(project): Project,
        ApiJson(request): ApiJson<RecordingFinish>,
    ) -> Result<Json<EpisodeInfo>, OperationError> {
        let (keep,) = request.into_parts();
        Ok(Json(TakeOperations::new(&project).stop(keep).await?))
    }
    async fn set_recording_paused(
        Project(project): Project,
        ApiJson(request): ApiJson<PauseUpdate>,
    ) -> Result<Json<()>, OperationError> {
        let (paused,) = request.into_parts();
        TakeOperations::new(&project).set_paused(paused).await?;
        Ok(Json(()))
    }
    async fn inspect_episode(
        Project(project): Project,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<EpisodeInspectionSnapshot>, OperationError> {
        Ok(Json(EpisodeCatalog::new(&project).inspection(id)?))
    }
}
