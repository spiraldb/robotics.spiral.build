//! Recording requests and their query invalidation policy. Episodes are
//! recorded on the robot connector, which uploads each finished one to
//! Studio; a browser stores it in its dataset.
use crate::client::http::UrlComponent;
use crate::client::queries::QueryDomain::*;
use crate::client::queries::QueryPolicy;
use crate::client::robot_connector::RobotConnector;
use crate::client::transport::{CommandError, HttpRequest};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::recording::{
    EpisodeInfo, EpisodeInspectionSnapshot, PauseUpdate, RecordingFinish, RecordingStart,
    SavedRecording,
};
use std::collections::BTreeSet;
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
                format!("/recording/episodes/{}/camera", episode.url_component()),
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
            format!("/recording/episodes/{id}", id = id.url_component()),
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

/// How often Rollout asks Studio for the episodes the robot connector uploaded.
const WAITING_EPISODES_POLL_MS: u32 = 2_000;

/// The finished episodes Studio holds until a browser stores each in its
/// dataset. While Rollout is active this browser stores each as it arrives,
/// once; one that fails waits for the operator to try again.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct EpisodeStorage {
    waiting: Resource<Result<Vec<SavedRecording>, CommandError>>,
    /// Being stored by this browser now, by episode.
    storing: Signal<BTreeSet<String>>,
    /// Stored or tried by this browser, by episode.
    tried: Signal<BTreeSet<String>>,
}
impl EpisodeStorage {
    pub(super) fn use_storage(project: StudioProjectState, is_active: ReadSignal<bool>) -> Self {
        let waiting = QueryPolicy::new(Recording, WAITING_EPISODES_POLL_MS).use_when(
            move || *is_active.read(),
            || RecordingTransfer::current().waiting(),
        );
        let storage = use_hook(|| Self {
            waiting,
            storing: Signal::new(BTreeSet::new()),
            tried: Signal::new(BTreeSet::new()),
        });
        use_effect(move || {
            let Some(Ok(waiting)) = &*waiting.read() else {
                return;
            };
            for saved in waiting {
                if !storage.tried.peek().contains(saved.episode_id()) {
                    storage.store(project, saved.clone());
                }
            }
        });
        storage
    }

    /// The episodes that wait and that this browser is not storing now: each
    /// one it tried failed. None until Studio has answered.
    pub(super) fn unstored(self) -> Option<Vec<SavedRecording>> {
        let waiting = self.waiting.read();
        let Some(Ok(saved)) = &*waiting else {
            return None;
        };
        let storing = self.storing.read();
        Some(
            saved
                .iter()
                .filter(|saved| !storing.contains(saved.episode_id()))
                .cloned()
                .collect(),
        )
    }

    /// Store `saved` in its dataset, reporting a failure.
    pub(super) fn store(mut self, project: StudioProjectState, saved: SavedRecording) {
        let episode = saved.episode_id().to_owned();
        self.storing.write().insert(episode.clone());
        self.tried.write().insert(episode.clone());
        project.run_reported("", async move {
            let stored = RecordingTransfer::current()
                .store(saved.dataset_id(), &episode)
                .await;
            self.storing.write().remove(&episode);
            stored.map_err(|error| format!("Could not store episode {episode} in Studio: {error}"))
        });
    }
}

/// Studio's side of finished episodes: the ones the robot connector uploaded,
/// and storing one in its dataset with this browser's Spiral access.
struct RecordingTransfer {
    connector: RobotConnector,
}
impl RecordingTransfer {
    fn current() -> Self {
        Self {
            connector: RobotConnector::current(),
        }
    }

    fn waiting(&self) -> impl Future<Output = Result<Vec<SavedRecording>, CommandError>> + use<> {
        self.connector.request(HttpRequest::new("GET", "/episodes"))
    }

    fn store(
        &self,
        dataset: &str,
        episode: &str,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        let (dataset, episode) = (dataset.url_component(), episode.url_component());
        self.connector.mutate(
            &[Recording, Catalog],
            HttpRequest::new("POST", format!("/episodes/{dataset}/{episode}/store")),
        )
    }
}
