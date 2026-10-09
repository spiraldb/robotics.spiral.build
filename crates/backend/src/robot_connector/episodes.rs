//! The episodes a workspace's robot connector uploads, part by part, and keeps
//! until a signed-in browser stores each in its dataset with its user's
//! Spiral access.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post, put},
};
use spiral_robotics_common::recording::SavedRecording;
use spiral_robotics_common::robot_connector::{EPISODE_PART_BYTES, EpisodePart, EpisodeUpload};
use spiral_robotics_storage::{EpisodeName, EpisodeReceipt, StagedEpisodeError};

use super::http::LinkedWorkspace;
use crate::OperationError;
use crate::http::delivery::{ApiBytes, ApiPath, ApiQuery, Project};
use crate::projects::context::StudioContext;

pub(crate) struct EpisodeEndpoints;

impl EpisodeEndpoints {
    /// Where a connector uploads a finished episode.
    pub(super) fn link() -> Router<StudioContext> {
        Router::new().route(
            "/robot-connector/link/episodes/{dataset}/{episode}",
            put(Self::receive).layer(DefaultBodyLimit::max(EPISODE_PART_BYTES)),
        )
    }

    pub(super) fn reads() -> Router<StudioContext> {
        Router::new().route("/robot-connector/episodes", get(Self::waiting))
    }

    pub(super) fn commands() -> Router<StudioContext> {
        Router::new().route(
            "/robot-connector/episodes/{dataset}/{episode}/store",
            post(Self::store),
        )
    }

    /// Keep one part of an episode: 200 once kept, 409 when it starts elsewhere
    /// than where the episode's bytes end. Either way the answer says how many
    /// bytes Studio holds.
    async fn receive(
        LinkedWorkspace(project_id): LinkedWorkspace,
        State(state): State<StudioContext>,
        ApiPath((dataset, episode)): ApiPath<(String, String)>,
        ApiQuery(part): ApiQuery<EpisodePart>,
        ApiBytes(bytes): ApiBytes,
    ) -> Result<(StatusCode, Json<EpisodeUpload>), OperationError> {
        let name = EpisodeName::new(&dataset, &episode).map_err(OperationError::InvalidInput)?;
        let receipt = state
            .robot_connectors()
            .episodes()
            .receive(&project_id, &name, part, &bytes)
            .await
            .map_err(|error| match error {
                StagedEpisodeError::Refused(reason) => OperationError::InvalidInput(reason),
                StagedEpisodeError::Storage(reason) => OperationError::Internal(reason),
            })?;
        Ok(match receipt {
            EpisodeReceipt::Kept(received) => (StatusCode::OK, Json(EpisodeUpload::new(received))),
            EpisodeReceipt::Misplaced(received) => {
                (StatusCode::CONFLICT, Json(EpisodeUpload::new(received)))
            }
        })
    }

    /// The workspace's whole episodes, waiting to be stored.
    async fn waiting(
        Project(project): Project,
    ) -> Result<Json<Vec<SavedRecording>>, OperationError> {
        let project_id = project.workspace_project_id()?;
        Ok(Json(
            project
                .studio()
                .robot_connectors()
                .episodes()
                .whole(&project_id)
                .await
                .map_err(OperationError::Internal)?,
        ))
    }

    /// Store a whole episode in its dataset with this browser's Spiral access,
    /// then forget it. Storing the same episode again only refreshes the
    /// dataset, so two browsers storing it at once is harmless.
    async fn store(
        Project(project): Project,
        ApiPath((dataset, episode)): ApiPath<(String, String)>,
    ) -> Result<Json<()>, OperationError> {
        let project_id = project.workspace_project_id()?;
        let name = EpisodeName::new(&dataset, &episode).map_err(OperationError::InvalidInput)?;
        let staged = project.studio().robot_connectors().episodes();
        let bytes = staged
            .read(&project_id, &name)
            .await
            .map_err(OperationError::Internal)?
            .ok_or_else(|| {
                OperationError::NotFound(format!("No whole episode {dataset}/{episode} waits"))
            })?;
        project
            .storage()
            .store_recording(&name, &bytes)
            .await
            .map_err(OperationError::External)?;
        staged
            .remove(&project_id, &name)
            .await
            .map_err(OperationError::Internal)?;
        Ok(Json(()))
    }
}
