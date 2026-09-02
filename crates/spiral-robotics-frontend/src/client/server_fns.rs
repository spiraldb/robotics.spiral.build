//! Fullstack training and rollout commands.

use std::collections::BTreeMap;

use dioxus::fullstack::FileStream;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_api::datasets::{
    EpisodeImportSummary, HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection,
    HuggingFaceDatasetSource, ProjectImportSummary,
};
use spiral_robotics_api::recording::{
    DatasetSummary, EpisodeInfo, EpisodeInspectionSnapshot, EpisodeListItem,
};
use spiral_robotics_api::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_api::robots::{
    CameraBinding, CameraStatus, ModeSelection, NewRobot, RobotSummary, RobotUpdate,
    SerialPortSummary,
};
use spiral_robotics_api::rollout::{RolloutConfig, RolloutSnapshot};
use spiral_robotics_api::settings::{
    DataSettings, SpiralConnectionUpdate, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_api::training::{TrainingConfig, TrainingRun};
use spiral_robotics_api::workflow::{
    WorkflowAgentCommand, WorkflowAgentSnapshot, WorkflowCommand, WorkflowSnapshot,
};

#[cfg(feature = "server")]
use dioxus::server::axum::Extension;
#[cfg(feature = "server")]
use spiral_robotics_backend::DeviceCommands;
#[cfg(feature = "server")]
use spiral_robotics_backend::RecordingCommands;
#[cfg(feature = "server")]
use spiral_robotics_backend::TrainingRolloutCommands;
#[cfg(feature = "server")]
use spiral_robotics_backend::WorkflowCommands;
#[cfg(feature = "server")]
use spiral_robotics_backend::{DataCommands, DataSettingsChange};

/// Message-only command failure transported as Dioxus typed error details.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CommandError(String);

pub(crate) struct EpisodeImportUpload {
    dataset_id: String,
    document: FileStream,
}

impl EpisodeImportUpload {
    pub(crate) const fn new(dataset_id: String, document: FileStream) -> Self {
        Self {
            dataset_id,
            document,
        }
    }
}

impl dioxus::fullstack::IntoRequest for EpisodeImportUpload {
    fn into_request(
        self,
        request: dioxus::fullstack::ClientRequest,
    ) -> impl std::future::Future<
        Output = Result<dioxus::fullstack::ClientResponse, dioxus::fullstack::RequestError>,
    > + 'static {
        async move {
            let request = request.header("X-Spiral-Dataset-ID", self.dataset_id)?;
            dioxus::fullstack::IntoRequest::into_request(self.document, request).await
        }
    }
}

impl dioxus::fullstack::extract::FromRequest<dioxus::fullstack::FullstackContext>
    for EpisodeImportUpload
{
    type Rejection = ServerFnError;

    fn from_request(
        request: dioxus::fullstack::extract::Request,
        state: &dioxus::fullstack::FullstackContext,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let dataset_id = request
            .headers()
            .get("X-Spiral-Dataset-ID")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let document = <FileStream as dioxus::fullstack::extract::FromRequest<_>>::from_request(
            request, state,
        );
        async move {
            let dataset_id = dataset_id.ok_or_else(|| {
                ServerFnError::Serialization("episode import is missing its dataset ID".to_owned())
            })?;
            Ok(Self::new(dataset_id, document.await?))
        }
    }
}

impl From<ServerFnError> for CommandError {
    fn from(error: ServerFnError) -> Self {
        match error {
            ServerFnError::ServerError { message, .. } => Self(message),
            error => Self(error.to_string()),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(feature = "server")]
impl CommandError {
    async fn read_upload(mut upload: FileStream, max_bytes: usize) -> Result<Vec<u8>, Self> {
        use futures_util::StreamExt as _;

        let file_name = upload.file_name().to_owned();
        if upload.size().is_some_and(|size| size > max_bytes as u64) {
            return Err(Self(format!(
                "{file_name} exceeds the {} MiB import limit",
                max_bytes / (1024 * 1024)
            )));
        }
        let capacity = upload
            .size()
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or_default()
            .min(max_bytes);
        let mut document = Vec::with_capacity(capacity);
        while let Some(chunk) = upload.next().await {
            let chunk = chunk.map_err(|error| Self(error.to_string()))?;
            let received = document
                .len()
                .checked_add(chunk.len())
                .filter(|received| *received <= max_bytes)
                .ok_or_else(|| {
                    Self(format!(
                        "{file_name} exceeds the {} MiB import limit",
                        max_bytes / (1024 * 1024)
                    ))
                })?;
            document.reserve(received - document.len());
            document.extend_from_slice(&chunk);
        }
        Ok(document)
    }
}

#[cfg(feature = "server")]
impl dioxus::fullstack::AsStatusCode for CommandError {
    fn as_status_code(&self) -> dioxus::fullstack::StatusCode {
        dioxus::fullstack::StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[server(
    endpoint = "server-functions/training/start",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn start_training_run(
    config: TrainingConfig,
) -> Result<TrainingRun, CommandError> {
    commands
        .0
        .start_training_run(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/training/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_training_run(id: String) -> Result<(), CommandError> {
    commands
        .0
        .stop_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/training/delete",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn delete_training_run(id: String) -> Result<(), CommandError> {
    commands
        .0
        .delete_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/rollout/start",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn start_rollout(config: RolloutConfig) -> Result<RolloutSnapshot, CommandError> {
    commands
        .0
        .start_rollout(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/rollout/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_rollout() -> Result<(), CommandError> {
    commands
        .0
        .stop_rollout()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/snapshot",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn workflow_snapshot() -> Result<WorkflowSnapshot, CommandError> {
    commands
        .0
        .snapshot()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/command",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn run_workflow_command(
    command: WorkflowCommand,
) -> Result<WorkflowSnapshot, CommandError> {
    commands
        .0
        .command(command)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/agent-snapshot",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn workflow_agent_snapshot() -> Result<WorkflowAgentSnapshot, CommandError> {
    commands
        .0
        .agent_snapshot()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/agent-command",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn run_workflow_agent_command(
    command: WorkflowAgentCommand,
) -> Result<WorkflowAgentSnapshot, CommandError> {
    commands
        .0
        .agent_command(command)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/list-datasets",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn list_datasets() -> Result<Vec<DatasetSummary>, CommandError> {
    commands
        .0
        .datasets()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/list-episodes",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn list_episodes(
    dataset_id: String,
) -> Result<(Vec<EpisodeListItem>, Vec<String>), CommandError> {
    commands
        .0
        .episodes(dataset_id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/start",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn start_recording(dataset_id: String) -> Result<EpisodeInfo, CommandError> {
    commands
        .0
        .start_recording(dataset_id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/stop",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn stop_recording(keep: bool) -> Result<EpisodeInfo, CommandError> {
    commands
        .0
        .stop_recording(keep)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/set-paused",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn set_recording_paused(paused: bool) -> Result<(), CommandError> {
    commands
        .0
        .set_recording_paused(paused)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/delete-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn delete_episode(id: String) -> Result<(), CommandError> {
    commands
        .0
        .delete_episode(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/inspect-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn inspect_episode(id: String) -> Result<EpisodeInspectionSnapshot, CommandError> {
    commands
        .0
        .inspection(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/seek-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn seek_episode(
    id: String,
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
) -> Result<(), CommandError> {
    commands
        .0
        .seek_episode(id, robot, source_arm, seconds)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/start-replay",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn start_replay(
    episode: String,
    robot: String,
    source_arm: Option<String>,
    start_seconds: f64,
) -> Result<(), CommandError> {
    commands
        .0
        .start_replay(episode, robot, source_arm, start_seconds)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/set-replay-paused",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn set_replay_paused(paused: bool) -> Result<(), CommandError> {
    commands
        .0
        .set_replay_paused(paused)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/list-ports",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn list_serial_ports() -> Result<Vec<SerialPortSummary>, CommandError> {
    commands
        .0
        .list_ports()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/identify-port",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn identify_serial_port(
    before: Vec<String>,
) -> Result<(Option<String>, String), CommandError> {
    commands
        .0
        .identify_port(before)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/refresh-cameras",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn refresh_cameras() -> Result<Vec<CameraStatus>, CommandError> {
    commands
        .0
        .refresh_cameras()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/list-robots",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn list_robots() -> Result<Vec<RobotSummary>, CommandError> {
    commands
        .0
        .list_robots()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/create-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn create_robot(request: NewRobot) -> Result<RobotSummary, CommandError> {
    commands
        .0
        .create_robot(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/import-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn import_robot_configuration(
    document: Vec<u8>,
) -> Result<RobotSummary, CommandError> {
    commands
        .0
        .import_robot_configuration(document)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/get-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn get_robot(id: String) -> Result<RobotSummary, CommandError> {
    commands
        .0
        .get_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/update-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn update_robot(
    request: RobotUpdate<Vec<CameraBinding>>,
) -> Result<RobotSummary, CommandError> {
    commands
        .0
        .update_robot(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/delete-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn delete_robot(id: String) -> Result<(), CommandError> {
    commands
        .0
        .delete_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/connect-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn connect_robot(id: String) -> Result<(), CommandError> {
    commands
        .0
        .connect_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/disconnect-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn disconnect_robot(id: String) -> Result<(), CommandError> {
    commands
        .0
        .disconnect_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/hold-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn hold_robot(id: String) -> Result<(), CommandError> {
    commands
        .0
        .hold_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/release-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn release_robot(id: String) -> Result<(), CommandError> {
    commands
        .0
        .release_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/set-robot-goal",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn set_robot_goal(
    id: String,
    normalized: BTreeMap<String, f64>,
) -> Result<(), CommandError> {
    commands
        .0
        .set_robot_goal(id, normalized)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/start-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn start_calibration(id: String) -> Result<(), CommandError> {
    commands
        .0
        .start_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/finish-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn finish_calibration(id: String) -> Result<RobotSummary, CommandError> {
    commands
        .0
        .finish_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/cancel-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn cancel_calibration(id: String) -> Result<(), CommandError> {
    commands
        .0
        .cancel_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/reapply-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn reapply_calibration(
    id: String,
) -> Result<(u64, u64, u64, u64, Vec<String>), CommandError> {
    commands
        .0
        .reapply_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/set-mode",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn set_mode(selection: ModeSelection) -> Result<(), CommandError> {
    commands
        .0
        .set_mode(selection)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/settings",
    commands: Extension<DataCommands>
)]
pub(crate) async fn data_settings() -> Result<DataSettings, CommandError> {
    commands
        .0
        .settings()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/update-settings",
    commands: Extension<DataCommands>
)]
pub(crate) async fn update_data_settings(
    connection: SpiralConnectionUpdate,
    training: TrainingRuntimeSettings,
    video: VideoSettings,
) -> Result<DataSettings, CommandError> {
    commands
        .0
        .update_settings(DataSettingsChange::new(connection, training, video))
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/test-spiral",
    commands: Extension<DataCommands>
)]
pub(crate) async fn test_spiral_connection() -> Result<(bool, String), CommandError> {
    commands
        .0
        .test_spiral()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/test-training-runtime",
    commands: Extension<DataCommands>
)]
pub(crate) async fn test_training_runtime() -> Result<(bool, String), CommandError> {
    commands
        .0
        .test_training_runtime()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/relations",
    commands: Extension<DataCommands>
)]
pub(crate) async fn relation_catalog() -> Result<RelationCatalog, CommandError> {
    commands
        .0
        .relations()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/query",
    commands: Extension<DataCommands>
)]
pub(crate) async fn run_spiral_query(
    statement: String,
    max_rows: usize,
) -> Result<SpiralQuerySnapshot, CommandError> {
    commands
        .0
        .query(statement, max_rows)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/inspect-hugging-face",
    commands: Extension<DataCommands>
)]
pub(crate) async fn inspect_hugging_face_dataset(
    request: HuggingFaceDatasetSource,
) -> Result<HuggingFaceDatasetInspection, CommandError> {
    commands
        .0
        .inspect_hugging_face_dataset(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/import-hugging-face",
    commands: Extension<DataCommands>
)]
pub(crate) async fn import_hugging_face_dataset(
    request: HuggingFaceDatasetSource,
) -> Result<HuggingFaceDatasetImportSummary, CommandError> {
    commands
        .0
        .import_hugging_face_dataset(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/import-project",
    commands: Extension<DataCommands>
)]
pub(crate) async fn import_project(
    document: FileStream,
) -> Result<ProjectImportSummary, CommandError> {
    let document =
        CommandError::read_upload(document, spiral_robotics_backend::MAX_PROJECT_IMPORT_BYTES)
            .await?;
    commands
        .0
        .import_project(document)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/import-episodes",
    commands: Extension<DataCommands>
)]
pub(crate) async fn import_episodes(
    upload: EpisodeImportUpload,
) -> Result<EpisodeImportSummary, CommandError> {
    let EpisodeImportUpload {
        dataset_id,
        document,
    } = upload;
    let document =
        CommandError::read_upload(document, spiral_robotics_backend::MAX_EPISODE_IMPORT_BYTES)
            .await?;
    commands
        .0
        .import_episodes(dataset_id, document)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}
