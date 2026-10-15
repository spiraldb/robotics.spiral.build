//! One instant of one recording as a picture: the still every episode row
//! leads with, in Data's rail and in every Curation index alike.
//!
//! A list of a hundred ids is a list you have to read; a list of a hundred
//! pictures is one you can scan. Each still is one episode query that reads
//! one camera's packets from the keyframe before its frame up to that frame,
//! decoded in the browser as the camera's panel decodes them, and the same
//! picture is asked for again and again: an episode's opening frame leads its
//! row in Data, in the Dataset node, and in every Select below it, and a
//! node's index is rebuilt whenever the operator comes back to it. So a still
//! is decoded once per project, camera, episode and instant, and every row
//! that shows it reads the same answer.
use super::decoder::{CameraPlayback, CameraStill, EncodedCamera};
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, StreamCatalog};
use crate::components::scoped_task::ScopedTask;
use dioxus::dioxus_core::{ScopeId, current_scope_id};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetSelection, DatasetStreams, EpisodeId, TopicRevision,
};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use std::{collections::HashMap, rc::Rc};

/// How long after its instant a still's frame may have been logged: the
/// frame is the first the camera logged from the instant on, among those it
/// logged within a second of it.
const FRAME_SEARCH_NANOS: u64 = 1_000_000_000;

/// The most packets a still's read may list. It lists one group of pictures
/// at most, far fewer: MCAP cameras record a keyframe at least every second,
/// LeRobot ones every 30 frames.
const STILL_PACKET_ROWS: usize = 1_000;

/// The largest slot a still fills, in CSS pixels: a Curation index row's.
/// Every other slot is smaller, and each crops the still to its own shape.
const STILL_SLOT: (f64, f64) = (96.0, 54.0);

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
            dataset: self.episode.dataset().clone(),
            camera: camera.to_owned(),
            episode: self.episode.id().clone(),
            // A millisecond is finer than any camera's frame interval.
            at_ms: (self.at * 1000.0).round() as u64,
        }
    }
    /// When the episode's first message was logged: the clock its instants
    /// count from.
    fn start_ns(&self) -> Result<u64, String> {
        self.episode
            .facts()
            .message_start_time()
            .ok_or_else(|| "This episode records no start to find a frame from.".to_owned())
    }
    /// The statement listing the packets of `camera`, one of the dataset's
    /// camera streams, that decode this instant's frame: the first the camera
    /// logged from the instant on, and every packet back to the keyframe at
    /// or before it, in decode order.
    ///
    /// Columns are named as a windowed read of a camera names them, so the
    /// packets are read as the camera's panel reads its own. The search's
    /// end bounds the packets too, so the read never reaches past it.
    fn packets_statement(&self, camera: &TopicRevision) -> Result<String, String> {
        let instant = self.start_ns()?.saturating_add((self.at * 1e9) as u64);
        let until = instant.saturating_add(FRAME_SEARCH_NANOS);
        let messages = self.episode.dataset().messages(camera);
        Ok(format!(
            "WITH shown AS (SELECT MIN(f.ordinal) AS ordinal FROM {messages} f \
             WHERE f.id = $1 AND f.log_time >= {instant} AND f.log_time < {until}), \
             group_start AS (SELECT MAX(k.ordinal) AS ordinal FROM {messages} k \
             WHERE k.id = $1 AND k.keyframe AND k.ordinal <= (SELECT ordinal FROM shown)) \
             SELECT r.log_time, r.value.format AS field_0, r.value.data AS field_1 FROM {messages} r \
             WHERE r.id = $1 AND r.log_time < {until} \
             AND r.ordinal BETWEEN (SELECT ordinal FROM group_start) AND (SELECT ordinal FROM shown) \
             ORDER BY r.ordinal"
        ))
    }
    /// This instant's frame on `camera`, decoded from its packets and drawn
    /// to fill any still's slot.
    async fn decode(&self, camera: &TopicRevision) -> Result<CameraStill, String> {
        let request = EpisodeQueryRequest::new(
            self.episode.selection(),
            self.packets_statement(camera)?,
            STILL_PACKET_ROWS,
        );
        let packets = EncodedCamera::query(&request, self.start_ns()?)
            .await
            .map_err(|error| format!("Decoding a still: {error}"))?;
        // The read ends on the frame the still shows.
        let shown = packets
            .last_frame_seconds()
            .ok_or("The camera logged no frame at this moment.")?;
        CameraPlayback::new(Rc::new(packets))
            .still(shown, STILL_SLOT)
            .await
    }
    /// The width and height this instant's frame on `camera` displays at:
    /// what the camera records at.
    pub(in crate::activities) async fn frame_size(
        &self,
        camera: &TopicRevision,
    ) -> Result<(u32, u32), String> {
        Ok(self.decode(camera).await?.frame_size())
    }
    /// This instant's frame on the camera topic `camera`, as a picture a row
    /// can show.
    async fn picture(self, camera: String, datasets: DatasetClient) -> Result<String, String> {
        let inspection = datasets
            .inspect_dataset(self.episode.dataset().clone())
            .await
            .map_err(|error| format!("Decoding a still: {error}"))?;
        let stream = DatasetStreams::new(inspection.topics())
            .camera(&camera)?
            .revision()
            .clone();
        Ok(self.decode(&stream).await?.url().to_owned())
    }
}

/// What one decoded still is filed under.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StillKey {
    dataset: DatasetSelection,
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
            match moment.picture(camera, datasets).await {
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

    fn top() -> TopicRevision {
        TopicRevision::new("top_0a1b2c3d").unwrap()
    }

    #[test]
    fn a_still_reads_its_frame_and_the_packets_back_to_the_keyframe_before_it() {
        // The fixture's recording starts one second into its log.
        let episode = Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000);
        let messages = "\"datasets\".\"folding\".\"messages\".\"top_0a1b2c3d\"";
        assert_eq!(
            StillMoment::at(&episode, 2.5)
                .packets_statement(&top())
                .expect("the episode records its start"),
            format!(
                "WITH shown AS (SELECT MIN(f.ordinal) AS ordinal FROM {messages} f \
                 WHERE f.id = $1 AND f.log_time >= 3500000000 AND f.log_time < 4500000000), \
                 group_start AS (SELECT MAX(k.ordinal) AS ordinal FROM {messages} k \
                 WHERE k.id = $1 AND k.keyframe AND k.ordinal <= (SELECT ordinal FROM shown)) \
                 SELECT r.log_time, r.value.format AS field_0, r.value.data AS field_1 FROM {messages} r \
                 WHERE r.id = $1 AND r.log_time < 4500000000 \
                 AND r.ordinal BETWEEN (SELECT ordinal FROM group_start) AND (SELECT ordinal FROM shown) \
                 ORDER BY r.ordinal"
            )
        );
        // An episode's opening still is its first frame from the episode's
        // first message on.
        let opening = StillMoment::opening(&episode)
            .packets_statement(&top())
            .expect("the episode records its start");
        assert!(opening.contains("f.log_time >= 1000000000 AND f.log_time < 2000000000"));
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
