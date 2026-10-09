//! Recording requests and their query invalidation policy. Takes are
//! recorded on the robot connector; finished ones are carried to Studio.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::robot_connector::RobotConnector;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingFinish, RecordingStart,
    SavedRecording,
};
use std::future::Future;

pub(crate) struct RecordingClient {
    connector: RobotConnector,
}
impl RecordingClient {
    pub(crate) fn camera_preview(
        &self,
        episode: String,
        stream: String,
    ) -> impl Future<Output = Result<spiral_robotics_common::datasets::EpisodeVideo, CommandError>> + use<>
    {
        self.connector.request(
            HttpRequest::new(
                "POST",
                format!("/recording/takes/{}/camera", episode.url_component()),
            )
            .json(&spiral_robotics_common::recording::RecordingCameraRequest::new(stream)),
        )
    }

    pub(super) fn inspect_episode(
        &self,
        id: String,
    ) -> impl Future<Output = Result<EpisodeInspectionSnapshot, CommandError>> + use<> {
        self.connector.request(HttpRequest::new(
            "GET",
            format!("/recording/takes/{id}", id = id.url_component()),
        ))
    }

    pub(crate) fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
        }
    }
    pub(super) fn recording_window(
        &self,
        cursor: spiral_robotics_common::recording_signals::RecordingCursor,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::recording_signals::RecordingWindow, CommandError>,
    > + use<> {
        self.connector
            .request(HttpRequest::new("GET", "/recording/signals").query(&cursor))
    }

    pub(super) fn start_recording(
        &self,
        request: RecordingStart,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, Recording, RobotSetup],
            HttpRequest::new("POST", "/recording").json(&request),
        )
    }

    pub(super) fn stop_recording(
        &self,
        keep: bool,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, Recording, Catalog, RobotSetup],
            HttpRequest::new("POST", "/recording/stop").json(&RecordingFinish::new(keep)),
        )
    }

    pub(super) fn set_recording_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.connector.mutate(
            &[Telemetry, Recording],
            HttpRequest::new("PATCH", "/recording").json(&PauseUpdate::new(paused)),
        )
    }
}

/// Carries finished takes from the robot connector into their Studio datasets:
/// each is read from the robot connector, stored by Studio, and only then
/// forgotten by the robot connector.
pub(super) struct RecordingTransfer {
    connector: RobotConnector,
    project: ProjectClient,
}
impl RecordingTransfer {
    pub(super) fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
            project: ProjectClient::current(),
        }
    }

    /// The takes the robot connector still holds.
    pub(super) fn waiting(
        &self,
    ) -> impl Future<Output = Result<Vec<SavedRecording>, CommandError>> + use<> {
        self.connector
            .request(HttpRequest::new("GET", "/recordings"))
    }

    pub(super) async fn carry(&self, dataset: &str, episode: &str) -> Result<(), CommandError> {
        let (dataset, episode) = (dataset.url_component(), episode.url_component());
        let saved = format!("/recordings/{dataset}/{episode}");
        let recording = self
            .connector
            .request_blob(
                HttpRequest::new("GET", saved.clone()),
                "application/octet-stream",
            )
            .await?;
        self.project
            .mutate::<()>(
                &[Catalog],
                HttpRequest::new(
                    "POST",
                    format!("/api/datasets/{dataset}/recordings/{episode}"),
                )
                .blob(recording),
            )
            .await?;
        self.connector
            .mutate(&[Recording], HttpRequest::new("DELETE", saved))
            .await
    }
}
