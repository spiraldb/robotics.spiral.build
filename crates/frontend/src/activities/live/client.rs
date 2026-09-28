//! Recording requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingFinish, RecordingStart,
};
use std::future::Future;

pub(crate) struct RecordingClient {
    project: ProjectClient,
}
impl RecordingClient {
    pub(crate) fn camera_preview(
        &self,
        episode: String,
        stream: String,
    ) -> impl Future<Output = Result<spiral_robotics_common::datasets::EpisodeVideo, CommandError>> + use<>
    {
        self.project.request(
            HttpRequest::new(
                "POST",
                format!("/api/recording/takes/{}/camera", episode.url_component()),
            )
            .json(&spiral_robotics_common::recording::RecordingCameraRequest::new(stream)),
        )
    }

    pub(super) fn inspect_episode(
        &self,
        id: String,
    ) -> impl Future<Output = Result<EpisodeInspectionSnapshot, CommandError>> + use<> {
        self.project.request(HttpRequest::new(
            "GET",
            format!("/api/recording/takes/{id}", id = id.url_component()),
        ))
    }

    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(super) fn recording_window(
        &self,
        cursor: spiral_robotics_common::recording_signals::RecordingCursor,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::recording_signals::RecordingWindow, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/recording/signals").query(&cursor))
    }

    pub(super) fn start_recording(
        &self,
        request: RecordingStart,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording, RobotSetup],
            HttpRequest::new("POST", "/api/recording").json(&request),
        )
    }

    pub(super) fn stop_recording(
        &self,
        keep: bool,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording, Catalog, RobotSetup],
            HttpRequest::new("POST", "/api/recording/stop").json(&RecordingFinish::new(keep)),
        )
    }

    pub(super) fn set_recording_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording],
            HttpRequest::new("PATCH", "/api/recording").json(&PauseUpdate::new(paused)),
        )
    }
}
