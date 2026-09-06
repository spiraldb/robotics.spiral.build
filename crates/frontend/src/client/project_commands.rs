//! Project-scoped command gateway. Each request captures its originating UI lifetime.

use std::collections::BTreeMap;
use std::future::Future;

use dioxus::fullstack::FileStream;
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

use super::project::ProjectClient;
use super::queries::QueryDomain::*;
use super::transport;
use super::transport::CommandError;

impl ProjectClient {
    pub(crate) fn setup_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::status::StudioSnapshot, CommandError>> + use<>
    {
        self.request(transport::fetch_setup)
    }
    pub(crate) fn telemetry_snapshot(
        &self,
    ) -> impl Future<Output = Result<spiral_robotics_common::robots::TelemetryFrame, CommandError>> + use<>
    {
        self.request(transport::fetch_telemetry)
    }
    pub(crate) fn recording_window(
        &self,
        cursor: spiral_robotics_common::recording_signals::RecordingCursor,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::recording_signals::RecordingWindow, CommandError>,
    > + use<> {
        self.request(move |id| transport::fetch_recording_signals(id, cursor))
    }
    pub(crate) fn training_snapshot(
        &self,
        selected: Option<String>,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::training::TrainingSnapshot, CommandError>,
    > + use<> {
        self.request(move |id| transport::fetch_training(id, selected))
    }
    pub(crate) fn rollout_snapshot(
        &self,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.request(transport::fetch_rollout)
    }

    pub(crate) fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> impl Future<Output = Result<TrainingRun, CommandError>> + use<> {
        self.mutate(&[Training], move |session_id| {
            transport::start_training_run(session_id, config)
        })
    }

    pub(crate) fn stop_training_run(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Training], move |session_id| {
            transport::stop_training_run(session_id, id)
        })
    }

    pub(crate) fn delete_training_run(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Training, Catalog], move |session_id| {
            transport::delete_training_run(session_id, id)
        })
    }

    pub(crate) fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> impl Future<Output = Result<RolloutSnapshot, CommandError>> + use<> {
        self.mutate(&[Rollout, Setup, Telemetry], move |session_id| {
            transport::start_rollout(session_id, config)
        })
    }

    pub(crate) fn stop_rollout(&self) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Rollout, Setup, Telemetry], transport::stop_rollout)
    }

    pub(crate) fn workflow_snapshot(
        &self,
    ) -> impl Future<Output = Result<WorkflowSnapshot, CommandError>> + use<> {
        self.request(transport::workflow_snapshot)
    }

    pub(crate) fn run_workflow_command(
        &self,
        command: WorkflowCommand,
    ) -> impl Future<Output = Result<WorkflowSnapshot, CommandError>> + use<> {
        self.mutate(&[Workflow, Catalog], move |session_id| {
            transport::run_workflow_command(session_id, command)
        })
    }

    pub(crate) fn workflow_agent_snapshot(
        &self,
    ) -> impl Future<Output = Result<WorkflowAgentSnapshot, CommandError>> + use<> {
        self.request(transport::workflow_agent_snapshot)
    }

    pub(crate) fn run_workflow_agent_command(
        &self,
        command: WorkflowAgentCommand,
    ) -> impl Future<Output = Result<WorkflowAgentSnapshot, CommandError>> + use<> {
        self.mutate(&[Assistant, Workflow, Catalog], move |session_id| {
            transport::run_workflow_agent_command(session_id, command)
        })
    }

    pub(crate) fn list_datasets(
        &self,
    ) -> impl Future<Output = Result<Vec<DatasetSummary>, CommandError>> + use<> {
        self.request(transport::list_datasets)
    }

    pub(crate) fn list_episodes(
        &self,
        dataset_id: String,
    ) -> impl Future<Output = Result<(Vec<EpisodeListItem>, Vec<String>), CommandError>> + use<>
    {
        self.request(move |session_id| transport::list_episodes(session_id, dataset_id))
    }

    pub(crate) fn start_recording(
        &self,
        dataset_id: String,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.mutate(&[Telemetry, Recording, Setup], move |session_id| {
            transport::start_recording(session_id, dataset_id)
        })
    }

    pub(crate) fn stop_recording(
        &self,
        keep: bool,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.mutate(
            &[Telemetry, Recording, Data, Catalog, Setup],
            move |session_id| transport::stop_recording(session_id, keep),
        )
    }

    pub(crate) fn set_recording_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry, Recording], move |session_id| {
            transport::set_recording_paused(session_id, paused)
        })
    }

    pub(crate) fn delete_episode(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Data, Catalog], move |session_id| {
            transport::delete_episode(session_id, id)
        })
    }

    pub(crate) fn inspect_episode(
        &self,
        id: String,
    ) -> impl Future<Output = Result<EpisodeInspectionSnapshot, CommandError>> + use<> {
        self.request(move |session_id| transport::inspect_episode(session_id, id))
    }

    pub(crate) fn seek_episode(
        &self,
        id: String,
        robot: String,
        source_arm: Option<String>,
        seconds: f64,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::seek_episode(session_id, id, robot, source_arm, seconds)
        })
    }

    pub(crate) fn start_replay(
        &self,
        episode: String,
        robot: String,
        source_arm: Option<String>,
        start_seconds: f64,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::start_replay(session_id, episode, robot, source_arm, start_seconds)
        })
    }

    pub(crate) fn set_replay_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry], move |session_id| {
            transport::set_replay_paused(session_id, paused)
        })
    }

    pub(crate) fn list_serial_ports(
        &self,
    ) -> impl Future<Output = Result<Vec<SerialPortSummary>, CommandError>> + use<> {
        self.request(transport::list_serial_ports)
    }

    pub(crate) fn identify_serial_port(
        &self,
        before: Vec<String>,
    ) -> impl Future<Output = Result<(Option<String>, String), CommandError>> + use<> {
        self.request(move |session_id| transport::identify_serial_port(session_id, before))
    }

    pub(crate) fn refresh_cameras(
        &self,
    ) -> impl Future<Output = Result<Vec<CameraStatus>, CommandError>> + use<> {
        self.mutate(&[Setup], transport::refresh_cameras)
    }

    pub(crate) fn create_robot(
        &self,
        request: NewRobot,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.mutate(&[Setup, Catalog], move |session_id| {
            transport::create_robot(session_id, request)
        })
    }

    pub(crate) fn import_robot_configuration(
        &self,
        document: Vec<u8>,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.mutate(&[Setup, Catalog], move |session_id| {
            transport::import_robot_configuration(session_id, document)
        })
    }

    pub(crate) fn update_robot(
        &self,
        request: RobotUpdate<Vec<CameraBinding>>,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.mutate(&[Setup], move |session_id| {
            transport::update_robot(session_id, request)
        })
    }

    pub(crate) fn delete_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Setup, Catalog], move |session_id| {
            transport::delete_robot(session_id, id)
        })
    }

    pub(crate) fn connect_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Setup, Telemetry], move |session_id| {
            transport::connect_robot(session_id, id)
        })
    }

    pub(crate) fn disconnect_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Setup, Telemetry], move |session_id| {
            transport::disconnect_robot(session_id, id)
        })
    }

    pub(crate) fn hold_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry], move |session_id| {
            transport::hold_robot(session_id, id)
        })
    }

    pub(crate) fn release_robot(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry], move |session_id| {
            transport::release_robot(session_id, id)
        })
    }

    pub(crate) fn set_robot_goal(
        &self,
        id: String,
        normalized: BTreeMap<String, f64>,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry], move |session_id| {
            transport::set_robot_goal(session_id, id, normalized)
        })
    }

    pub(crate) fn start_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::start_calibration(session_id, id)
        })
    }

    pub(crate) fn finish_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<RobotSummary, CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::finish_calibration(session_id, id)
        })
    }

    pub(crate) fn cancel_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::cancel_calibration(session_id, id)
        })
    }

    pub(crate) fn reapply_calibration(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(u64, u64, u64, u64, Vec<String>), CommandError>> + use<> {
        self.mutate(&[Telemetry, Setup], move |session_id| {
            transport::reapply_calibration(session_id, id)
        })
    }

    pub(crate) fn set_mode(
        &self,
        selection: ModeSelection,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.mutate(&[Setup, Telemetry], move |session_id| {
            transport::set_mode(session_id, selection)
        })
    }

    pub(crate) fn data_settings(
        &self,
    ) -> impl Future<Output = Result<DataSettings, CommandError>> + use<> {
        self.request(transport::data_settings)
    }

    pub(crate) fn update_data_settings(
        &self,
        connection: SpiralConnectionUpdate,
        training: TrainingRuntimeSettings,
        video: VideoSettings,
    ) -> impl Future<Output = Result<DataSettings, CommandError>> + use<> {
        self.mutate(
            &[Project, Settings, Setup, Training, Assistant, Catalog, Data],
            move |session_id| {
                transport::update_data_settings(session_id, connection, training, video)
            },
        )
    }

    pub(crate) fn test_spiral_connection(
        &self,
    ) -> impl Future<Output = Result<(bool, String), CommandError>> + use<> {
        self.mutate(
            &[Project, Settings, Setup, Catalog, Data],
            transport::test_spiral_connection,
        )
    }

    pub(crate) fn test_training_runtime(
        &self,
    ) -> impl Future<Output = Result<(bool, String), CommandError>> + use<> {
        self.mutate(&[Training], transport::test_training_runtime)
    }

    pub(crate) fn relation_catalog(
        &self,
    ) -> impl Future<Output = Result<RelationCatalog, CommandError>> + use<> {
        self.request(transport::relation_catalog)
    }

    pub(crate) fn run_spiral_query(
        &self,
        statement: String,
        max_rows: usize,
    ) -> impl Future<Output = Result<SpiralQuerySnapshot, CommandError>> + use<> {
        self.request(move |session_id| transport::run_spiral_query(session_id, statement, max_rows))
    }

    pub(crate) fn inspect_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> impl Future<Output = Result<HuggingFaceDatasetInspection, CommandError>> + use<> {
        self.request(move |session_id| transport::inspect_hugging_face_dataset(session_id, request))
    }

    pub(crate) fn import_hugging_face_dataset(
        &self,
        request: HuggingFaceDatasetSource,
    ) -> impl Future<Output = Result<HuggingFaceDatasetImportSummary, CommandError>> + use<> {
        self.mutate(&[Data, Catalog], move |session_id| {
            transport::import_hugging_face_dataset(session_id, request)
        })
    }

    pub(crate) fn import_project(
        &self,
        document: FileStream,
    ) -> impl Future<Output = Result<ProjectImportSummary, CommandError>> + use<> {
        self.mutate(
            &[
                Project, Settings, Catalog, Data, Training, Workflow, Assistant,
            ],
            move |session_id| {
                transport::import_project(super::project_upload::ProjectUpload::new(
                    session_id, document,
                ))
            },
        )
    }
}
