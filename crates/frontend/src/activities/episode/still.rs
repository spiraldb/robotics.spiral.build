//! One instant of one recording as a picture: the still every episode row
//! leads with, in Data's rail and in every Curation index alike.
//!
//! A list of a hundred ids is a list you have to read; a list of a hundred
//! pictures is one you can scan. Each still is one Spiral query that seeks one
//! camera to one instant and decodes the frame there, about a second of
//! server time, and the same picture is asked for again and again: an
//! episode's opening frame leads its row in Data, in the Dataset node, and in
//! every Select below it, and a node's index is rebuilt whenever the operator
//! comes back to it. So a still is decoded once per project, camera, episode
//! and instant, and every row that shows it reads the same answer.
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, StreamCatalog};
use crate::components::scoped_task::ScopedTask;
use dioxus::dioxus_core::{ScopeId, current_scope_id};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;
use std::collections::HashMap;

/// How long after its instant a still's frame may have been logged.
///
/// Bounding the read on both sides is what lets Spiral seek: it decodes the
/// group of pictures that holds the instant, where an open-ended `ORDER BY
/// log_time LIMIT 1` decodes the rest of the episode to sort it. A camera that
/// logged nothing in the second after an instant has no still for it.
const FRAME_SEARCH_NANOS: u64 = 1_000_000_000;

/// Which picture a still is: an instant of one episode.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct StillMoment {
    episode: Episode,
    /// Seconds from the episode's first message.
    at: f64,
}

impl StillMoment {
    /// A whole episode's still: its first frame.
    pub(in crate::activities) fn opening(episode: &Episode) -> Self {
        Self::at(episode, 0.0)
    }
    /// The frame `seconds` into `episode`.
    pub(in crate::activities) fn at(episode: &Episode, seconds: f64) -> Self {
        Self {
            episode: episode.clone(),
            at: seconds.max(0.0),
        }
    }
    fn key(&self, camera: &str) -> StillKey {
        StillKey {
            dataset: self.episode.dataset().sql_name(),
            camera: camera.to_owned(),
            episode: self.episode.id().clone(),
            // A millisecond is finer than any camera's frame interval.
            at_ms: (self.at * 1000.0).round() as u64,
        }
    }
    /// The statement that decodes this instant's frame on `camera`: the first
    /// one the camera logged from the instant on.
    ///
    /// `read_video` seeks only on a literal log-time bound, so the instant is
    /// placed on the episode's clock here rather than joined to it in SQL. The
    /// recordings carry no colour metadata; the assumed profile reproduces the
    /// picture FFmpeg decodes from them.
    fn statement(&self, camera: &str) -> Result<String, String> {
        let start = self
            .episode
            .facts()
            .message_start_time()
            .ok_or("This episode records no start to find a frame from.")?;
        let from = start.saturating_add((self.at * 1e9) as u64);
        Ok(format!(
            "SELECT spql.image.to_rgb(v.value, 'srgb_bt709_limited') AS image \
             FROM spql.mcap.read_video({}, {}) v \
             WHERE v.id = $1 AND v.log_time >= {from} AND v.log_time < {} \
             ORDER BY v.log_time LIMIT 1",
            SqlLiteral::text(&self.episode.dataset().sql_name()),
            SqlLiteral::text(camera),
            from.saturating_add(FRAME_SEARCH_NANOS),
        ))
    }
    /// This instant's frame on `camera`, as a picture a row can show.
    async fn decode(self, camera: String, datasets: DatasetClient) -> Result<String, String> {
        let request =
            EpisodeQueryRequest::new(self.episode.selection(), self.statement(&camera)?, 1);
        let snapshot = datasets
            .run_episode_query(&request)
            .await
            .map_err(|error| format!("Decoding a still: {error}"))?;
        let image = snapshot
            .columns()
            .iter()
            .find(|column| column.name() == "image")
            .ok_or("The still's query returned no image.")?;
        snapshot
            .rows()
            .first()
            .and_then(|row| image.image_data_url(row.get(image.name())))
            .map(str::to_owned)
            .ok_or_else(|| "The camera logged no frame at this moment.".to_owned())
    }
}

/// What one decoded still is filed under.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StillKey {
    dataset: String,
    camera: String,
    episode: EpisodeId,
    at_ms: u64,
}

/// What a still's slot shows right now.
#[derive(Clone, Debug, PartialEq)]
enum StillState {
    Waiting,
    Picture(String),
    Failed(String),
}

impl StillState {
    /// What the slot reports to the style sheet.
    fn name(&self) -> &'static str {
        match self {
            Self::Picture(_) => "ready",
            Self::Waiting => "waiting",
            Self::Failed(_) => "failed",
        }
    }
}

/// Every still this project has decoded, or is decoding, shared by every row
/// that shows one. A handle, provided once per project.
///
/// Each still is its own signal, so a picture arriving repaints the rows that
/// show it rather than every row of every index.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EpisodeStills {
    decoded: Signal<HashMap<StillKey, Signal<StillState>>>,
    /// The project's scope: decodes run as its tasks, so one outlives the row
    /// that asked for it and stops with the project.
    owner: ScopeId,
}

impl EpisodeStills {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            decoded: Signal::new(HashMap::new()),
            owner: current_scope_id(),
        })
    }
    fn current() -> Self {
        consume_context()
    }
    /// The camera every list of `dataset`'s episodes shows its stills on:
    /// the one that shows the workspace best, among the cameras the
    /// project's inspection of the dataset names.
    pub(in crate::activities) fn use_overview_camera(
        dataset: Option<DatasetSelection>,
    ) -> Option<String> {
        let recorded = StreamCatalog::use_camera_topics(dataset);
        StreamCatalog::overview_camera(recorded.as_deref()?).map(str::to_owned)
    }
    /// The still of `moment` on `camera` if another row already asked for
    /// it, without asking: a row that mounts over a decoded picture shows it
    /// on its first frame rather than after it has been seen.
    fn known(self, moment: &StillMoment, camera: Option<&str>) -> Option<Signal<StillState>> {
        self.decoded.peek().get(&moment.key(camera?)).copied()
    }
    /// The still of `moment` on `camera`: the answer another row already has
    /// or is waiting for, else a decode started now.
    ///
    /// A failed still is not kept, so the next row to ask tries again; the
    /// row that saw it fail keeps saying why.
    fn request(
        self,
        moment: &StillMoment,
        camera: &str,
        datasets: DatasetClient,
    ) -> Signal<StillState> {
        let key = moment.key(camera);
        if let Some(known) = self.decoded.peek().get(&key) {
            return *known;
        }
        let mut state = Signal::new_in_scope(StillState::Waiting, self.owner);
        let mut decoded = self.decoded;
        decoded.write().insert(key.clone(), state);
        let (moment, camera) = (moment.clone(), camera.to_owned());
        ScopedTask::spawn(self.owner, async move {
            match moment.decode(camera, datasets).await {
                Ok(picture) => state.set(StillState::Picture(picture)),
                Err(reason) => {
                    decoded.write().remove(&key);
                    state.set(StillState::Failed(reason));
                }
            }
        });
        state
    }
}

/// One row's still: the picture once it is decoded, the recess until then.
///
/// A row asks for its picture when it has first been seen and its camera is
/// known, whichever comes last, and again when its instant changes; until the
/// new picture arrives it keeps showing the old one, so moving a threshold
/// re-decodes under pictures rather than under a column of blanks.
#[component]
pub(in crate::activities) fn EpisodeStill(
    moment: StillMoment,
    /// The camera the list is about; `None` until it is known.
    camera: Option<String>,
) -> Element {
    let stills = EpisodeStills::current();
    let datasets = use_hook(DatasetClient::current);
    let mut seen = use_signal(|| false);
    let mut asked = use_signal(|| stills.known(&moment, camera.as_deref()));
    let mut previous = use_signal(|| None::<String>);
    use_effect(use_reactive((&moment, &camera), move |(moment, camera)| {
        let (Some(camera), true) = (camera, seen()) else {
            return;
        };
        asked.set(Some(stills.request(&moment, &camera, datasets.clone())));
    }));
    let state = asked().map_or(StillState::Waiting, |state| state());
    use_effect(use_reactive((&state,), move |(state,)| {
        if let StillState::Picture(picture) = state {
            previous.set(Some(picture));
        }
    }));
    let shown = match (state, previous()) {
        (StillState::Waiting, Some(picture)) => StillState::Picture(picture),
        (state, _) => state,
    };
    let state_name = shown.name();
    rsx! {
        div {
            class: "episode-still",
            "data-state": state_name,
            onvisible: move |event| {
                if !*seen.peek() && event.data().is_intersecting().unwrap_or(true) {
                    seen.set(true);
                }
            },
            match shown {
                StillState::Picture(picture) => rsx! {
                    img { src: "{picture}", alt: "", decoding: "async" }
                },
                // A still being decoded is a skeleton, not a reason.
                StillState::Waiting => rsx! {
                    span { class: "episode-still-empty", "aria-hidden": "true" }
                },
                // A frame that failed says so in the row, in words a reader
                // reaches.
                StillState::Failed(reason) => rsx! {
                    span { class: "episode-still-empty", title: "{reason}", "{reason}" }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_seeks_the_camera_to_its_instant_on_the_episodes_clock() {
        // The fixture's recording starts one second into its log.
        let episode = Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000);
        let opening = StillMoment::opening(&episode)
            .statement("/top")
            .expect("the episode records its start");
        assert!(opening.contains("FROM spql.mcap.read_video('folding', '/top') v"));
        assert!(opening.contains(
            "WHERE v.id = $1 AND v.log_time >= 1000000000 AND v.log_time < 2000000000 \
             ORDER BY v.log_time LIMIT 1"
        ));
        let middle = StillMoment::at(&episode, 2.5)
            .statement("/top")
            .expect("the episode records its start");
        assert!(middle.contains("v.log_time >= 3500000000 AND v.log_time < 4500000000"));
    }

    #[test]
    fn one_instant_on_one_camera_is_filed_once() {
        let episode = Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000);
        let early = StillMoment::at(&episode, 1.2344);
        assert_eq!(
            early.key("/top"),
            StillMoment::at(&episode, 1.2341).key("/top")
        );
        assert_ne!(early.key("/top"), early.key("/wrist"));
        assert_ne!(
            early.key("/top"),
            StillMoment::opening(&episode).key("/top")
        );
    }
}
