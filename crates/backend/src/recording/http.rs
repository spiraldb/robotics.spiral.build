//! HTTP endpoints for episodes.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use crate::http::contract::{ApiRoutes, Created};
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, EpisodeSeek, PauseUpdate, RecordingFinish,
    RecordingStart, ReplayStart,
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
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/recording/signals", Self::fetch_recording_signals);
        routes.post("/recording", Self::start_recording);
        routes.patch("/recording", Self::set_recording_paused);
        routes.post("/recording/stop", Self::stop_recording);
        routes.delete("/recording/takes/{id}", Self::delete_episode);
        routes.get("/recording/takes/{id}", Self::inspect_episode);
        routes.post("/recording/takes/{id}/camera", Self::camera_preview);
        routes.post("/recording/takes/{id}/seek", Self::seek_episode);
        routes.post("/replay", Self::start_replay);
        routes.patch("/replay", Self::set_replay_paused);
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
    async fn seek_episode(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<EpisodeSeek>,
    ) -> Result<Json<()>, HttpFailure> {
        let (robot, source_arm, seconds) = request.into_parts();
        commands
            .seek_episode(id, robot, source_arm, seconds)
            .await?;
        Ok(Json(()))
    }
    async fn start_replay(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<ReplayStart>,
    ) -> Result<Json<()>, HttpFailure> {
        let (episode, robot, source_arm, start_seconds) = request.into_parts();
        commands
            .start_replay(episode, robot, source_arm, start_seconds)
            .await?;
        Ok(Json(()))
    }
    async fn set_replay_paused(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<PauseUpdate>,
    ) -> Result<Json<()>, HttpFailure> {
        let (paused,) = request.into_parts();
        commands.set_replay_paused(paused).await?;
        Ok(Json(()))
    }
}
