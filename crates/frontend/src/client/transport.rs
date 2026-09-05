//! Fullstack training and rollout commands.

use std::collections::BTreeMap;

#[cfg(feature = "server")]
use dioxus::fullstack::FileStream;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{
    HuggingFaceDatasetImportSummary, HuggingFaceDatasetInspection, HuggingFaceDatasetSource,
    ProjectImportSummary,
};
use spiral_robotics_common::recording::{
    DatasetSummary, EpisodeInfo, EpisodeInspectionSnapshot, EpisodeListItem,
};
use spiral_robotics_common::relations::{RelationCatalog, SpiralQuerySnapshot};
use spiral_robotics_common::robots::{
    CameraBinding, CameraStatus, ModeSelection, NewRobot, RobotSummary, RobotUpdate,
    SerialPortSummary,
};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
use spiral_robotics_common::settings::{
    DataSettings, SpiralConnectionUpdate, TrainingRuntimeSettings, VideoSettings,
};
use spiral_robotics_common::training::{TrainingConfig, TrainingRun};
use spiral_robotics_common::workflow::{
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

impl CommandError {
    pub(crate) fn stale_project() -> Self {
        Self("The project changed before this request completed. Refresh the current project and try again.".to_owned())
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
    session_id: String,
    config: TrainingConfig,
) -> Result<TrainingRun, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .start_training_run(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/training/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_training_run(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .stop_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/training/delete",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn delete_training_run(
    session_id: String,
    id: String,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .delete_training_run(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/rollout/start",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn start_rollout(
    session_id: String,
    config: RolloutConfig,
) -> Result<RolloutSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .start_rollout(config)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/rollout/stop",
    commands: Extension<TrainingRolloutCommands>
)]
pub(crate) async fn stop_rollout(session_id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .stop_rollout()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/snapshot",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn workflow_snapshot(
    session_id: String,
) -> Result<WorkflowSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .snapshot()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/command",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn run_workflow_command(
    session_id: String,
    command: WorkflowCommand,
) -> Result<WorkflowSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .command(command)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/agent-snapshot",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn workflow_agent_snapshot(
    session_id: String,
) -> Result<WorkflowAgentSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .agent_snapshot()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/workflow/agent-command",
    commands: Extension<WorkflowCommands>
)]
pub(crate) async fn run_workflow_agent_command(
    session_id: String,
    command: WorkflowAgentCommand,
) -> Result<WorkflowAgentSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .agent_command(command)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/list-datasets",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn list_datasets(session_id: String) -> Result<Vec<DatasetSummary>, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .datasets()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/list-episodes",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn list_episodes(
    session_id: String,
    dataset_id: String,
) -> Result<(Vec<EpisodeListItem>, Vec<String>), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .episodes(dataset_id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/start",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn start_recording(
    session_id: String,
    dataset_id: String,
) -> Result<EpisodeInfo, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .start_recording(dataset_id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/stop",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn stop_recording(
    session_id: String,
    keep: bool,
) -> Result<EpisodeInfo, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .stop_recording(keep)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/set-paused",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn set_recording_paused(
    session_id: String,
    paused: bool,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .set_recording_paused(paused)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/delete-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn delete_episode(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .delete_episode(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/inspect-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn inspect_episode(
    session_id: String,
    id: String,
) -> Result<EpisodeInspectionSnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .inspection(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/seek-episode",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn seek_episode(
    session_id: String,
    id: String,
    robot: String,
    source_arm: Option<String>,
    seconds: f64,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .seek_episode(id, robot, source_arm, seconds)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/start-replay",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn start_replay(
    session_id: String,
    episode: String,
    robot: String,
    source_arm: Option<String>,
    start_seconds: f64,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .start_replay(episode, robot, source_arm, start_seconds)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/recordings/set-replay-paused",
    commands: Extension<RecordingCommands>
)]
pub(crate) async fn set_replay_paused(
    session_id: String,
    paused: bool,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .set_replay_paused(paused)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/list-ports",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn list_serial_ports(
    session_id: String,
) -> Result<Vec<SerialPortSummary>, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .list_ports()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/identify-port",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn identify_serial_port(
    session_id: String,
    before: Vec<String>,
) -> Result<(Option<String>, String), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .identify_port(before)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/refresh-cameras",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn refresh_cameras(session_id: String) -> Result<Vec<CameraStatus>, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .refresh_cameras()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/list-robots",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn list_robots(session_id: String) -> Result<Vec<RobotSummary>, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .list_robots()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/create-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn create_robot(
    session_id: String,
    request: NewRobot,
) -> Result<RobotSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .create_robot(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/import-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn import_robot_configuration(
    session_id: String,
    document: Vec<u8>,
) -> Result<RobotSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .import_robot_configuration(document)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/get-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn get_robot(
    session_id: String,
    id: String,
) -> Result<RobotSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .get_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/update-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn update_robot(
    session_id: String,
    request: RobotUpdate<Vec<CameraBinding>>,
) -> Result<RobotSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .update_robot(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/delete-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn delete_robot(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .delete_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/connect-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn connect_robot(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .connect_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/disconnect-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn disconnect_robot(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .disconnect_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/hold-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn hold_robot(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .hold_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/release-robot",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn release_robot(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .release_robot(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/set-robot-goal",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn set_robot_goal(
    session_id: String,
    id: String,
    normalized: BTreeMap<String, f64>,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .set_robot_goal(id, normalized)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/start-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn start_calibration(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .start_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/finish-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn finish_calibration(
    session_id: String,
    id: String,
) -> Result<RobotSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .finish_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/cancel-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn cancel_calibration(session_id: String, id: String) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .cancel_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/reapply-calibration",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn reapply_calibration(
    session_id: String,
    id: String,
) -> Result<(u64, u64, u64, u64, Vec<String>), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .reapply_calibration(id)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/devices/set-mode",
    commands: Extension<DeviceCommands>
)]
pub(crate) async fn set_mode(
    session_id: String,
    selection: ModeSelection,
) -> Result<(), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .set_mode(selection)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/settings",
    commands: Extension<DataCommands>
)]
pub(crate) async fn data_settings(session_id: String) -> Result<DataSettings, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .settings()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/update-settings",
    commands: Extension<DataCommands>
)]
pub(crate) async fn update_data_settings(
    session_id: String,
    connection: SpiralConnectionUpdate,
    training: TrainingRuntimeSettings,
    video: VideoSettings,
) -> Result<DataSettings, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .update_settings(DataSettingsChange::new(connection, training, video))
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/test-spiral",
    commands: Extension<DataCommands>
)]
pub(crate) async fn test_spiral_connection(
    session_id: String,
) -> Result<(bool, String), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .test_spiral()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/test-training-runtime",
    commands: Extension<DataCommands>
)]
pub(crate) async fn test_training_runtime(
    session_id: String,
) -> Result<(bool, String), CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .test_training_runtime()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/relations",
    commands: Extension<DataCommands>
)]
pub(crate) async fn relation_catalog(session_id: String) -> Result<RelationCatalog, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .relations()
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/query",
    commands: Extension<DataCommands>
)]
pub(crate) async fn run_spiral_query(
    session_id: String,
    statement: String,
    max_rows: usize,
) -> Result<SpiralQuerySnapshot, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .query(statement, max_rows)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/inspect-hugging-face",
    commands: Extension<DataCommands>
)]
pub(crate) async fn inspect_hugging_face_dataset(
    session_id: String,
    request: HuggingFaceDatasetSource,
) -> Result<HuggingFaceDatasetInspection, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .inspect_hugging_face_dataset(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/import-hugging-face",
    commands: Extension<DataCommands>
)]
pub(crate) async fn import_hugging_face_dataset(
    session_id: String,
    request: HuggingFaceDatasetSource,
) -> Result<HuggingFaceDatasetImportSummary, CommandError> {
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    commands
        .import_hugging_face_dataset(request)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}

#[server(
    endpoint = "server-functions/data/import-project",
    commands: Extension<DataCommands>
)]
pub(crate) async fn import_project(
    upload: super::project_upload::ProjectUpload,
) -> Result<ProjectImportSummary, CommandError> {
    let (session_id, document) = upload.into_parts();
    let commands = commands
        .0
        .for_session(&session_id)
        .map_err(|failure| CommandError(failure.to_string()))?;
    let document =
        CommandError::read_upload(document, spiral_robotics_backend::MAX_PROJECT_IMPORT_BYTES)
            .await?;
    commands
        .import_project(document)
        .await
        .map_err(|failure| CommandError(failure.to_string()))
}
