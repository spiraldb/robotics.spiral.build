//! HTTP endpoints for takes: the one being recorded, and the finished
//! recordings waiting for the browser to carry them to Studio.
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingCameraRequest, RecordingFinish,
    RecordingStart, SavedRecording,
};
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};

use super::catalog::EpisodeCatalog;
use super::take::TakeOperations;
use crate::OperationError;
use crate::context::RobotConnectorContext;
use crate::http::delivery::{ApiJson, ApiPath, ApiQuery, Created};

use axum::{
    Json, Router,
    extract::State,
    http::header,
    response::IntoResponse,
    routing::{get, post},
};

pub(crate) struct RecordingEndpoints;

impl RecordingEndpoints {
    pub(crate) fn reads() -> Router<RobotConnectorContext> {
        Router::new()
            .route("/api/recording/signals", get(Self::fetch_recording_signals))
            .route("/api/recording/takes/{id}", get(Self::inspect_episode))
            .route(
                "/api/recording/takes/{id}/camera",
                post(Self::camera_preview),
            )
            .route("/api/recordings", get(Self::list_recordings))
            .route(
                "/api/recordings/{dataset}/{episode}",
                get(Self::download_recording),
            )
    }
    pub(crate) fn commands() -> Router<RobotConnectorContext> {
        Router::new()
            .route(
                "/api/recording",
                post(Self::start_recording).patch(Self::set_recording_paused),
            )
            .route("/api/recording/stop", post(Self::stop_recording))
            .route(
                "/api/recordings/{dataset}/{episode}",
                axum::routing::delete(Self::delete_recording),
            )
    }
    async fn list_recordings(
        State(connector): State<RobotConnectorContext>,
    ) -> Result<Json<Vec<SavedRecording>>, OperationError> {
        Ok(Json(connector.recording_files().list().await?))
    }
    async fn download_recording(
        State(connector): State<RobotConnectorContext>,
        ApiPath((dataset, episode)): ApiPath<(String, String)>,
    ) -> Result<impl IntoResponse, OperationError> {
        let bytes = connector.recording_files().read(&dataset, &episode).await?;
        Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes))
    }
    /// Studio has stored the recording; the robot connector forgets it.
    async fn delete_recording(
        State(connector): State<RobotConnectorContext>,
        ApiPath((dataset, episode)): ApiPath<(String, String)>,
    ) -> Result<Json<()>, OperationError> {
        connector
            .recording_files()
            .delete(&dataset, &episode)
            .await?;
        Ok(Json(()))
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
            EpisodeCatalog::new(&connector).camera_preview(id, request)?,
        ))
    }
    async fn start_recording(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<RecordingStart>,
    ) -> Result<Created<EpisodeInfo>, OperationError> {
        let summary = TakeOperations::new(&connector)
            .start(request.into())
            .await?;
        Ok(Created(summary))
    }
    async fn stop_recording(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<RecordingFinish>,
    ) -> Result<Json<EpisodeInfo>, OperationError> {
        let (keep,) = request.into_parts();
        Ok(Json(TakeOperations::new(&connector).stop(keep).await?))
    }
    async fn set_recording_paused(
        State(connector): State<RobotConnectorContext>,
        ApiJson(request): ApiJson<PauseUpdate>,
    ) -> Result<Json<()>, OperationError> {
        let (paused,) = request.into_parts();
        TakeOperations::new(&connector).set_paused(paused).await?;
        Ok(Json(()))
    }
    async fn inspect_episode(
        State(connector): State<RobotConnectorContext>,
        ApiPath(id): ApiPath<String>,
    ) -> Result<Json<EpisodeInspectionSnapshot>, OperationError> {
        Ok(Json(EpisodeCatalog::new(&connector).inspection(id)?))
    }
}
