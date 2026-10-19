//! One instant of one recording as a picture: the still every episode row
//! leads with, in Data's rail and in every Curation index alike.
//!
//! A list of a hundred ids is a list you have to read; a list of a hundred
//! pictures is one you can scan. A still is one camera's packets from the
//! keyframe before its frame up to that frame, decoded in the browser as the
//! camera's panel decodes them. An episode's opening still is one packet, its
//! camera's first, so the opening stills of the rows that come into view
//! together are read in one statement; a still at any other instant is one
//! episode query of its own. The same picture is asked for again and again:
//! an episode's opening frame leads its row in Data, in the Dataset node, and
//! in every Select below it, and a node's index is rebuilt whenever the
//! operator comes back to it. So a still is decoded once per project, camera,
//! episode and instant, and every row that shows it reads the same answer.
use super::decoder::{CameraPlayback, CameraStill, EncodedCamera};
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, StreamCatalog};
use crate::components::scoped_task::ScopedTask;
use dioxus::dioxus_core::{ScopeId, current_scope_id};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetSelection, DatasetStreams, EpisodeId, TopicRevision,
};
use spiral_robotics_common::relations::{EpisodeQueryRequest, EpisodesQueryRequest};
use std::{collections::HashMap, io::Cursor, rc::Rc};

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

/// How long an opening still waits for the rows that come into view with it,
/// so one statement reads all of their first frames.
const OPENING_GATHER_MS: u32 = 30;

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
    /// Whether it is its episode's opening still.
    fn opens(&self) -> bool {
        self.at == 0.0
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
        let shown = packets.last_frame_seconds().ok_or(Self::NO_FRAME)?;
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
        let stream =
            EpisodeStills::camera_stream(&datasets, self.episode.dataset(), &camera).await?;
        Ok(self.decode(&stream).await?.url().to_owned())
    }
}

/// The opening stills of episodes of one dataset on one camera, asked for
/// together. An opening still is almost always its camera's first packet: a
/// keyframe logged within a second of the episode's start, which decodes
/// alone, so one statement reads every asked episode's first packet. An
/// episode that statement does not answer is read the way a still at any
/// other instant is: one whose first packet is not such a keyframe, as an
/// MCAP camera logged out of order can be, or every asked episode when the
/// statement fails, as one over the response limit does.
struct OpeningStills {
    camera: String,
    asked: Vec<AskedStill>,
}

impl OpeningStills {
    /// `asked` by dataset and camera, in the order each pair was first asked.
    fn group(asked: Vec<AskedStill>) -> Vec<Self> {
        let mut groups = Vec::<Self>::new();
        for still in asked {
            match groups.iter_mut().find(|group| {
                group.camera == still.camera && group.dataset() == still.moment.episode.dataset()
            }) {
                Some(group) => group.asked.push(still),
                None => groups.push(Self {
                    camera: still.camera.clone(),
                    asked: vec![still],
                }),
            }
        }
        groups
    }
    fn dataset(&self) -> &DatasetSelection {
        self.asked[0].moment.episode.dataset()
    }
    /// Every asked episode's first packet on `camera`, `$1` through `$N`
    /// binding the episodes in the order they were asked.
    fn statement(&self, camera: &TopicRevision) -> String {
        let bound = (1..=self.asked.len())
            .map(|place| format!("${place}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "SELECT r.id AS episode_id, r.keyframe, r.log_time, r.value.format AS field_0, \
             r.value.data AS field_1 FROM {} r WHERE r.id IN ({bound}) AND r.ordinal = 0",
            self.dataset().messages(camera)
        )
    }
    /// Read every asked still and settle each one: its picture, or why it
    /// has none.
    async fn settle(
        self,
        datasets: DatasetClient,
        mut decoded: Signal<HashMap<StillKey, Signal<StillState>>>,
    ) {
        let packets = self.read(&datasets).await.unwrap_or_default();
        for mut still in self.asked {
            let alone = packets
                .get(still.moment.episode.id())
                .filter(|packet| still.moment.decodes_alone(packet));
            let picture = match alone {
                Some(packet) => still.moment.opening_picture(packet).await,
                None => {
                    let (moment, camera) = (still.moment.clone(), still.camera.clone());
                    moment.picture(camera, datasets.clone()).await
                }
            };
            match picture {
                Ok(picture) => still.state.set(StillState::Picture(picture)),
                Err(reason) => {
                    decoded.write().remove(&still.key);
                    still.state.set(StillState::Failed(reason));
                }
            }
        }
    }
    /// Each asked episode's first packet, as a one-row batch.
    async fn read(
        &self,
        datasets: &DatasetClient,
    ) -> Result<HashMap<EpisodeId, arrow_array::RecordBatch>, String> {
        let fail = |error: String| format!("Decoding a still: {error}");
        let stream = EpisodeStills::camera_stream(datasets, self.dataset(), &self.camera).await?;
        let request = EpisodesQueryRequest::new(
            self.dataset().clone(),
            self.asked
                .iter()
                .map(|still| still.moment.episode.id().clone())
                .collect(),
            self.statement(&stream),
            1,
        );
        let response = datasets
            .run_episodes_arrow_query(&request)
            .await
            .map_err(|error| fail(error.to_string()))?;
        let reader = arrow_ipc::reader::StreamReader::try_new(Cursor::new(response), None)
            .map_err(|error| fail(error.to_string()))?;
        let mut packets = HashMap::new();
        for batch in reader {
            let batch = batch.map_err(|error| fail(error.to_string()))?;
            let episodes = batch
                .column_by_name("episode_id")
                .and_then(|ids| ids.as_any().downcast_ref::<arrow_array::UInt64Array>())
                .ok_or_else(|| fail("a packet names no episode".to_owned()))?;
            for row in 0..batch.num_rows() {
                packets.insert(EpisodeId::new(episodes.value(row)), batch.slice(row, 1));
            }
        }
        Ok(packets)
    }
}

impl StillMoment {
    /// What a still says when its instant has no frame to show.
    const NO_FRAME: &'static str = "The camera logged no frame at this moment.";

    /// Whether `packet`, the episode's first on its camera, is its opening
    /// frame and decodes alone: a keyframe logged within a second of the
    /// episode's start. Then the frame the opening still shows is that
    /// packet, as the statement for any instant would find it.
    fn decodes_alone(&self, packet: &arrow_array::RecordBatch) -> bool {
        let logged = packet
            .column_by_name("log_time")
            .and_then(|times| times.as_any().downcast_ref::<arrow_array::UInt64Array>())
            .map(|times| times.value(0));
        let keyframe = packet
            .column_by_name("keyframe")
            .and_then(|keyframes| {
                keyframes
                    .as_any()
                    .downcast_ref::<arrow_array::BooleanArray>()
            })
            .is_some_and(|keyframes| keyframes.value(0));
        match (self.start_ns(), logged) {
            (Ok(start), Some(logged)) => {
                keyframe && logged < start.saturating_add(FRAME_SEARCH_NANOS)
            }
            _ => false,
        }
    }
    /// The opening picture of `packet`, a first packet that decodes alone.
    async fn opening_picture(&self, packet: &arrow_array::RecordBatch) -> Result<String, String> {
        let packets = EncodedCamera::from_messages(std::slice::from_ref(packet), self.start_ns()?)
            .map_err(|error| format!("Decoding a still: {error}"))?;
        let shown = packets.last_frame_seconds().ok_or(Self::NO_FRAME)?;
        Ok(CameraPlayback::new(Rc::new(packets))
            .still(shown, STILL_SLOT)
            .await?
            .url()
            .to_owned())
    }
}

/// A still a row asked for, waiting to be read with the others asked with it.
struct AskedStill {
    moment: StillMoment,
    camera: String,
    key: StillKey,
    state: Signal<StillState>,
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
    /// Opening stills asked for since the last were read, read together.
    openings: Signal<Vec<AskedStill>>,
    /// The project's scope: decodes run as its tasks, so one outlives the row
    /// that asked for it and stops with the project.
    owner: ScopeId,
}

impl EpisodeStills {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            decoded: Signal::new(HashMap::new()),
            openings: Signal::new(Vec::new()),
            owner: current_scope_id(),
        })
    }
    /// The stream of the camera topic `camera` in `dataset`, from the
    /// project's inspection of it.
    async fn camera_stream(
        datasets: &DatasetClient,
        dataset: &DatasetSelection,
        camera: &str,
    ) -> Result<TopicRevision, String> {
        let inspection = datasets
            .inspect_dataset(dataset.clone())
            .await
            .map_err(|error| format!("Decoding a still: {error}"))?;
        Ok(DatasetStreams::new(inspection.topics())
            .camera(camera)?
            .revision()
            .clone())
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
        if moment.opens() {
            let asked = AskedStill {
                moment,
                camera,
                key,
                state,
            };
            self.gather(asked, datasets);
            return state;
        }
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
    /// Keep `asked` with the opening stills asked for in the same moment; the
    /// first of them reads them all, by dataset and camera, once the rows
    /// that come into view with it have asked too.
    fn gather(self, asked: AskedStill, datasets: DatasetClient) {
        let mut openings = self.openings;
        let first = openings.peek().is_empty();
        openings.write().push(asked);
        if !first {
            return;
        }
        let (owner, decoded) = (self.owner, self.decoded);
        ScopedTask::spawn(owner, async move {
            gloo_timers::future::TimeoutFuture::new(OPENING_GATHER_MS).await;
            let asked = std::mem::take(&mut *openings.write());
            for stills in OpeningStills::group(asked) {
                ScopedTask::spawn(owner, stills.settle(datasets.clone(), decoded));
            }
        });
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
        let messages = "\"datasets.folding.messages.top_0a1b2c3d\"";
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

    /// Rows that come into view together read their opening frames in one
    /// statement for each dataset and camera.
    #[test]
    fn opening_stills_asked_together_read_one_packet_each_in_one_statement() {
        let asked = |id: u32, camera: &str| {
            let moment =
                StillMoment::opening(&Episode::fixture(&EpisodeId::fixture(id), 30_000_000_000));
            AskedStill {
                key: moment.key(camera),
                camera: camera.to_owned(),
                moment,
                state: Signal::new_in_scope(StillState::Waiting, ScopeId::ROOT),
            }
        };
        let mut dom = dioxus::dioxus_core::VirtualDom::new(|| rsx! {});
        dom.rebuild_in_place();
        dom.in_runtime(|| {
            let groups =
                OpeningStills::group(vec![asked(1, "/top"), asked(2, "/wrist"), asked(3, "/top")]);
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].asked.len(), 2);
            assert_eq!(
                groups[0].statement(&top()),
                "SELECT r.id AS episode_id, r.keyframe, r.log_time, r.value.format AS field_0, \
                 r.value.data AS field_1 FROM \"datasets.folding.messages.top_0a1b2c3d\" r \
                 WHERE r.id IN ($1, $2) AND r.ordinal = 0"
            );
        });
        assert!(StillMoment::opening(&Episode::fixture(&EpisodeId::fixture(1), 1)).opens());
        assert!(!StillMoment::at(&Episode::fixture(&EpisodeId::fixture(1), 1), 0.5).opens());
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
