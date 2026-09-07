//! Recording requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeSeek, PauseUpdate, RecordingFinish, RecordingStart, ReplayStart,
};
use std::future::Future;

pub(crate) struct RecordingClient {
    project: ProjectClient,
}
impl RecordingClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn recording_window(
        &self,
        cursor: spiral_robotics_common::recording_signals::RecordingCursor,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::recording_signals::RecordingWindow, CommandError>,
    > + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/recording/signals").query(&cursor))
    }

    pub(crate) fn start_recording(
        &self,
        dataset_id: String,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording, Setup],
            HttpRequest::new("POST", "/api/recording").json(&RecordingStart::new(dataset_id)),
        )
    }

    pub(crate) fn stop_recording(
        &self,
        keep: bool,
    ) -> impl Future<Output = Result<EpisodeInfo, CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording, Data, Catalog, Setup],
            HttpRequest::new("POST", "/api/recording/stop").json(&RecordingFinish::new(keep)),
        )
    }

    pub(crate) fn set_recording_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Recording],
            HttpRequest::new("PATCH", "/api/recording").json(&PauseUpdate::new(paused)),
        )
    }

    pub(crate) fn seek_episode(
        &self,
        id: String,
        request: EpisodeSeek,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Setup],
            HttpRequest::new(
                "POST",
                format!("/api/episodes/{id}/seek", id = id.url_component()),
            )
            .json(&request),
        )
    }

    pub(crate) fn start_replay(
        &self,
        request: ReplayStart,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry, Setup],
            HttpRequest::new("POST", "/api/replay").json(&request),
        )
    }

    pub(crate) fn set_replay_paused(
        &self,
        paused: bool,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Telemetry],
            HttpRequest::new("PATCH", "/api/replay").json(&PauseUpdate::new(paused)),
        )
    }
}
