//! HTTP endpoints for episodes.
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use crate::http::contract::{ApiRoutes, Created};
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, HttpFailure, ScopedProject};

use axum::Json;
use spiral_robotics_common::recording::{
    DatasetSummary, EpisodeCatalog, EpisodeInfo, EpisodeInspectionSnapshot, EpisodeSeek,
    PauseUpdate, RecordingFinish, RecordingStart, ReplayStart,
};

pub(crate) struct EpisodeEndpoints;

impl EpisodeEndpoints {
    async fn fetch_recording_signals(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiQuery(cursor): ApiQuery<RecordingCursor>,
    ) -> Result<Json<RecordingWindow>, HttpFailure> {
        let result = commands.recording_signals(cursor)?;
        Ok(Json(result))
    }
    pub(crate) fn register(routes: &mut ApiRoutes) {
        routes.get("/recording/signals", Self::fetch_recording_signals);
        routes.get("/datasets", Self::list_datasets);
        routes.get("/datasets/{dataset_id}/episodes", Self::list_episodes);
        routes.post("/recording", Self::start_recording);
        routes.patch("/recording", Self::set_recording_paused);
        routes.post("/recording/stop", Self::stop_recording);
        routes.delete("/episodes/{id}", Self::delete_episode);
        routes.get("/episodes/{id}", Self::inspect_episode);
        routes.post("/episodes/{id}/seek", Self::seek_episode);
        routes.post("/replay", Self::start_replay);
        routes.patch("/replay", Self::set_replay_paused);
    }
    async fn list_datasets(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
    ) -> Result<Json<Vec<DatasetSummary>>, HttpFailure> {
        let result = commands.datasets().await?;
        Ok(Json(result))
    }
    async fn list_episodes(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiPath(dataset_id): ApiPath<String>,
    ) -> Result<Json<EpisodeCatalog>, HttpFailure> {
        let result = commands.episodes(dataset_id).await?;
        Ok(Json(result))
    }
    async fn start_recording(
        ScopedProject(commands): ScopedProject<crate::recording::commands::RecordingCommands>,
        ApiJson(request): ApiJson<RecordingStart>,
    ) -> Result<Created<EpisodeInfo>, HttpFailure> {
        let (dataset_id,) = request.into_parts();
        let result = commands.start_recording(dataset_id).await?;
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
