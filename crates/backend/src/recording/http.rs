//! HTTP endpoints for episodes.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, Created, HttpFailure, ScopedProject};
use crate::projects::context::StudioContext;

use axum::{
    Json, Router,
    routing::{get, post},
};
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingFinish, RecordingStart,
};

pub(crate) struct RecordingEndpoints;

impl RecordingEndpoints {
    async fn fetch_recording_signals(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiQuery(cursor): ApiQuery<RecordingCursor>,
    ) -> Result<Json<RecordingWindow>, HttpFailure> {
        let result = commands.recording_signals(cursor)?;
        Ok(Json(result))
    }
    pub(crate) fn router() -> Router<StudioContext> {
        Router::new()
            .route("/recording/signals", get(Self::fetch_recording_signals))
            .route(
                "/recording",
                post(Self::start_recording).patch(Self::set_recording_paused),
            )
            .route("/recording/stop", post(Self::stop_recording))
            .route(
                "/recording/takes/{id}",
                get(Self::inspect_episode).delete(Self::delete_episode),
            )
            .route("/recording/takes/{id}/camera", post(Self::camera_preview))
    }

    async fn camera_preview(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<spiral_robotics_common::recording::RecordingCameraRequest>,
    ) -> Result<Json<spiral_robotics_common::datasets::EpisodeVideo>, HttpFailure> {
        Ok(Json(commands.camera_preview(id, request).await?))
    }

    async fn start_recording(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<RecordingStart>,
    ) -> Result<Created<EpisodeInfo>, HttpFailure> {
        let result = commands.start_recording(request).await?;
        Ok(Created(result))
    }
    async fn stop_recording(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<RecordingFinish>,
    ) -> Result<Json<EpisodeInfo>, HttpFailure> {
        let (keep,) = request.into_parts();
        let result = commands.stop_recording(keep).await?;
        Ok(Json(result))
    }
    async fn set_recording_paused(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<PauseUpdate>,
    ) -> Result<Json<()>, HttpFailure> {
        let (paused,) = request.into_parts();
        commands.set_recording_paused(paused).await?;
        Ok(Json(()))
    }
    async fn delete_episode(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<()>, HttpFailure> {
        commands.delete_episode(id).await?;
        Ok(Json(()))
    }
    async fn inspect_episode(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<EpisodeInspectionSnapshot>, HttpFailure> {
        let result = commands.inspection(id).await?;
        Ok(Json(result))
    }
}
