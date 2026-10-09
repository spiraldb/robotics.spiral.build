//! HTTP endpoints for episodes: the one being recorded, and the last one, for
//! review. Finished episodes go to Studio on the link.
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingCameraRequest, RecordingFinish,
    RecordingStart,
};
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use super::catalog::EpisodeCatalog;
use super::episode::EpisodeOperations;
use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, Created};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};

pub(crate) struct RecordingEndpoints;

impl RecordingEndpoints {
    pub(crate) fn reads() -> Router<RobotConnectorContext> {
        Router::new()
            .route("/api/recording/signals", get(Self::fetch_recording_signals))
            .route("/api/recording/episodes/{id}", get(Self::inspect_episode))
            .route(
                "/api/recording/episodes/{id}/camera",
                post(Self::camera_preview),
            )
    }
    pub(crate) fn commands() -> Router<RobotConnectorContext> {
        Router::new()
            .route(
                "/api/recording",
                post(Self::start_recording).patch(Self::set_recording_paused),
            )
            .route("/api/recording/stop", post(Self::stop_recording))
    }
    async fn fetch_recording_signals(
        State(connector): State<RobotConnectorContext>,
        ApiQuery(cursor): ApiQuery<RecordingCursor>,
    ) -> Json<RecordingWindow> {
        Json(connector.hardware().recording_signals(cursor))
    }
    async fn camera_preview(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
        ApiJson(request): ApiJson<RecordingCameraRequest>,
    ) -> Result<Json<EpisodeVideo>, OperationError> {
        Ok(Json(
            EpisodeCatalog::new(&connector)
                .camera_preview(id, request)
                .await?,
        ))
    }
    async fn start_recording(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<RecordingStart>,
    ) -> Result<Created<EpisodeInfo>, OperationError> {
        let summary = EpisodeOperations::new(&connector)
            .start(request.into())
            .await?;
        Ok(Created(summary))
    }
    async fn stop_recording(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<RecordingFinish>,
    ) -> Result<Json<EpisodeInfo>, OperationError> {
        let (keep,) = request.into_parts();
        Ok(Json(EpisodeOperations::new(&connector).stop(keep).await?))
    }
    async fn set_recording_paused(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<PauseUpdate>,
    ) -> Result<Json<()>, OperationError> {
        let (paused,) = request.into_parts();
        EpisodeOperations::new(&connector)
            .set_paused(paused)
            .await?;
        Ok(Json(()))
    }
    async fn inspect_episode(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<EpisodeInspectionSnapshot>, OperationError> {
        Ok(Json(EpisodeCatalog::new(&connector).inspection(id).await?))
    }
}
