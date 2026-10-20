//! The recorded streams of one episode: what each one is and how to read it.
//!
//! A stream is one topic in one schema inside one episode: an MCAP file's
//! channel, or a LeRobot episode's feature. Its kind is decided from what was
//! recorded — a `foxglove.CompressedVideo` schema, or a LeRobot camera's
//! `h264` — never from topic or field names.
use super::identity::Episode;
use crate::activities::data::DatasetClient;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetTopic, EpisodeSelection, EpisodeTopicRequest,
    MessageRead,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::activities) enum StreamKind {
    Video,
    Data,
}

#[derive(Clone, PartialEq, Debug)]
pub(in crate::activities) struct RecordedStream {
    key: String,
    label: String,
    topic: DatasetTopic,
    read: MessageRead,
}

impl RecordedStream {
    fn new(topic: DatasetTopic, episode: &EpisodeSelection, duplicated: bool) -> Self {
        let revision = topic.revision().clone();
        let key = revision.stream().to_owned();
        // A topic recorded as two streams is told apart by the hash its
        // stream's name ends with.
        let label = match key.rsplit_once('_') {
            Some((_, hash)) if duplicated => format!("{} · {hash}", topic.name()),
            _ => topic.name().to_owned(),
        };
        let read = MessageRead::schema(
            EpisodeTopicRequest::new(
                episode.dataset().clone(),
                topic.name().to_owned(),
                episode.episode_id().to_owned(),
            ),
            revision,
        );
        Self {
            key,
            label,
            topic,
            read,
        }
    }
    /// Its stream: stable within the dataset, and distinct for each schema
    /// of one topic.
    pub(in crate::activities) fn key(&self) -> &str {
        &self.key
    }
    pub(in crate::activities) fn label(&self) -> &str {
        &self.label
    }
    pub(in crate::activities) fn name(&self) -> &str {
        self.topic.name()
    }
    pub(in crate::activities) fn kind(&self) -> StreamKind {
        if self.topic.is_video() {
            StreamKind::Video
        } else {
            StreamKind::Data
        }
    }
    /// The native schema its messages were recorded with; a LeRobot
    /// feature has none.
    pub(in crate::activities) fn schema(&self) -> Option<&str> {
        self.topic.schema()
    }
    pub(in crate::activities) fn samples(&self) -> u64 {
        self.topic.samples()
    }
    pub(in crate::activities) fn read(&self) -> &MessageRead {
        &self.read
    }
}

/// Every stream of one episode, in topic order.
#[derive(Clone, PartialEq, Debug, Default)]
pub(in crate::activities) struct StreamCatalog {
    streams: Vec<RecordedStream>,
}

/// Camera names worth leading with: a view of the whole workspace shows what
/// the arm is doing to what, which a camera on the gripper cannot.
const OVERVIEW_NAMES: [&str; 4] = ["top", "front", "head", "overview"];
/// Camera names worth leading with last: they follow the gripper, so the scene
/// leaves the frame exactly when what is happening matters.
const CLOSE_UP_NAMES: [&str; 2] = ["wrist", "gripper"];

impl StreamCatalog {
    /// Which of these cameras shows the workspace best: an overview before an
    /// unremarkable name, and a wrist or gripper camera last. Every surface
    /// that leads with one camera out of several asks this.
    pub(in crate::activities) fn overview_camera(cameras: &[String]) -> Option<&str> {
        fn rank(camera: &str) -> u8 {
            let name = camera.to_lowercase();
            if OVERVIEW_NAMES.iter().any(|word| name.contains(word)) {
                0
            } else if CLOSE_UP_NAMES.iter().any(|word| name.contains(word)) {
                2
            } else {
                1
            }
        }
        // `min_by_key` keeps the first of equal ranks, so the dataset's own
        // topic order decides between two cameras that read alike.
        cameras
            .iter()
            .min_by_key(|camera| rank(camera))
            .map(String::as_str)
    }
    fn new(mut topics: Vec<DatasetTopic>, episode: &EpisodeSelection) -> Self {
        topics.sort_by(|left, right| {
            left.name()
                .cmp(right.name())
                .then_with(|| left.revision().cmp(right.revision()))
        });
        let mut names = BTreeMap::<String, usize>::new();
        for topic in &topics {
            *names.entry(topic.name().to_owned()).or_default() += 1;
        }
        let streams = topics
            .into_iter()
            .map(|topic| {
                let duplicated = names.get(topic.name()).is_some_and(|count| *count > 1);
                RecordedStream::new(topic, episode, duplicated)
            })
            .collect();
        Self { streams }
    }
    /// Every stream the episode recorded: the streams its listed row names,
    /// as the project's inspection of its dataset records them, which every
    /// surface showing the dataset has usually read already.
    pub(in crate::activities) async fn load(episode: Episode) -> Result<Self, String> {
        let streams = episode
            .facts()
            .streams()
            .ok_or("This episode was listed without its streams.")?;
        let topics = DatasetClient::current()
            .inspect_dataset(episode.dataset().clone())
            .await
            .map_err(|error| error.to_string())?
            .episode_topics(streams)?;
        Ok(Self::new(topics, &episode.selection()))
    }
    /// The camera topics `dataset` records, in its own topic order, from the
    /// project's inspection of it: every episode of a dataset carries the
    /// same cameras, so the dataset answers for each of them. `None` until
    /// the inspection is read, or when it cannot be; no dataset records no
    /// cameras.
    pub(in crate::activities) fn use_camera_topics(
        dataset: Option<DatasetSelection>,
    ) -> Option<Vec<String>> {
        // An inspection already read answers on the first frame, and is
        // always the current dataset's.
        let known = dataset
            .as_ref()
            .and_then(|dataset| DatasetClient::current().inspected(dataset))
            .map(|inspection| Self::cameras_of(&inspection));
        let cameras = use_resource(use_reactive(&dataset, move |dataset| async move {
            let Some(dataset) = dataset else {
                return Some(Vec::new());
            };
            let inspection = DatasetClient::current()
                .inspect_dataset(dataset)
                .await
                .ok()?;
            Some(Self::cameras_of(&inspection))
        }));
        known.or_else(|| cameras.read().clone().flatten())
    }
    /// The camera topics `inspection` names, each once, in its topic order.
    fn cameras_of(inspection: &DatasetInspection) -> Vec<String> {
        let mut cameras = Vec::<String>::new();
        for topic in inspection.topics().iter().filter(|topic| topic.is_video()) {
            if !cameras.iter().any(|camera| camera == topic.name()) {
                cameras.push(topic.name().to_owned());
            }
        }
        cameras
    }
    pub(in crate::activities) fn streams(&self) -> &[RecordedStream] {
        &self.streams
    }
    pub(in crate::activities) fn videos(&self) -> impl Iterator<Item = &RecordedStream> {
        self.streams
            .iter()
            .filter(|stream| stream.kind() == StreamKind::Video)
    }
    /// The part of a camera's name that tells it from the episode's other
    /// cameras: `/left-wrist-camera` beside `/top-camera` reads as
    /// "left-wrist" and "top". A lone camera keeps its whole name.
    pub(in crate::activities) fn camera_title(&self, camera: &RecordedStream) -> String {
        let names = self.videos().map(RecordedStream::name).collect::<Vec<_>>();
        Self::camera_title_among(camera.name(), &names)
    }
    /// What to call camera topic `name` among the episode's `cameras`: the
    /// part of its name the others do not share ("top", "left-wrist").
    pub(in crate::activities) fn camera_title_among(
        name: &str,
        cameras: &[impl AsRef<str>],
    ) -> String {
        let mut names = cameras.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        if !names.contains(&name) {
            names.push(name);
        }
        let distinct = if names.len() > 1 {
            let prefix = names.shared_prefix_len();
            let suffix = names.shared_suffix_len();
            name.get(prefix..name.len().saturating_sub(suffix))
                .unwrap_or("")
        } else {
            name
        };
        let distinct = distinct.trim_matches(|character: char| !character.is_alphanumeric());
        if distinct.is_empty() {
            name.trim_start_matches('/').to_owned()
        } else {
            distinct.to_owned()
        }
    }
    /// Data streams, busiest first; ties keep topic order.
    pub(in crate::activities) fn data_by_traffic(&self) -> Vec<RecordedStream> {
        let mut ranked = self
            .streams
            .iter()
            .filter(|stream| stream.kind() == StreamKind::Data)
            .cloned()
            .collect::<Vec<_>>();
        ranked.sort_by_key(|stream| std::cmp::Reverse(stream.samples()));
        ranked
    }
    pub(in crate::activities) fn get(&self, key: &str) -> Option<&RecordedStream> {
        self.streams.iter().find(|stream| stream.key() == key)
    }
    pub(in crate::activities) fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }
}

/// The bytes every name in a set shares at its start and at its end, on
/// character boundaries.
trait SharedAffixes {
    fn shared_prefix_len(&self) -> usize;
    fn shared_suffix_len(&self) -> usize;
}

impl SharedAffixes for [&str] {
    fn shared_prefix_len(&self) -> usize {
        let Some((first, rest)) = self.split_first() else {
            return 0;
        };
        let mut shared = first.len();
        for other in rest {
            let common = first
                .char_indices()
                .zip(other.chars())
                .take_while(|((_, left), right)| left == right)
                .last()
                .map_or(0, |((index, character), _)| index + character.len_utf8());
            shared = shared.min(common);
        }
        shared
    }
    fn shared_suffix_len(&self) -> usize {
        let Some((first, rest)) = self.split_first() else {
            return 0;
        };
        let mut shared = first.len();
        for other in rest {
            let common = first
                .chars()
                .rev()
                .zip(other.chars().rev())
                .take_while(|(left, right)| left == right)
                .map(|(character, _)| character.len_utf8())
                .sum::<usize>();
            shared = shared.min(common);
        }
        shared
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, TopicRevision};

    /// A dataset's stream of `name` in `schema`, as its `topics` table
    /// names it.
    fn topic(name: &str, stream: &str, schema: &str) -> DatasetTopic {
        DatasetTopic::new(
            name.into(),
            TopicRevision::new(stream).unwrap(),
            "protobuf".into(),
        )
        .with_schema(Some(schema.into()))
    }

    fn episode() -> EpisodeSelection {
        EpisodeSelection::new(
            DatasetSelection::new("default").unwrap(),
            EpisodeId::fixture(1),
        )
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn camera_titles_keep_only_what_differs_between_the_episode_cameras() {
        let titles = |names: &[&str]| {
            let topics = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    topic(
                        name,
                        &format!("camera_{index:08}"),
                        "foxglove.CompressedVideo",
                    )
                })
                .collect();
            let catalog = StreamCatalog::new(topics, &episode());
            catalog
                .videos()
                .map(|camera| catalog.camera_title(camera))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(&["/left-wrist-camera", "/right-wrist-camera", "/top-camera"]),
            ["left-wrist", "right-wrist", "top"]
        );
        assert_eq!(
            titles(&[
                "/camera/follower.camera.gripper/video",
                "/camera/follower.camera.top/video"
            ]),
            ["gripper", "top"]
        );
        assert_eq!(
            titles(&[
                "/cameras/left_camera-images-rgb",
                "/cameras/top_camera-images-rgb"
            ]),
            ["left", "top"]
        );
        assert_eq!(titles(&["/cam"]), ["cam"]);
        // Names that share everything but a suffix keep the whole name when
        // nothing else is left to show.
        assert_eq!(
            titles(&["/top-camera", "/top-camera-2"]),
            ["top-camera", "2"]
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn data_streams_rank_busiest_first_with_ties_in_topic_order() {
        let topics = vec![
            topic("/a", "a_11111111", "Sensor").with_coverage(1, 600),
            topic("/b", "b_22222222", "Sensor").with_coverage(1, 9_000),
            topic("/c", "c_33333333", "Sensor").with_coverage(1, 600),
            topic("/cam", "cam_44444444", "foxglove.CompressedVideo").with_coverage(1, 90_000),
        ];
        let ranked = StreamCatalog::new(topics, &episode())
            .data_by_traffic()
            .iter()
            .map(|stream| stream.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ranked, ["/b", "/a", "/c"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn streams_are_keyed_by_stream_and_typed_by_schema_name() {
        let topics = vec![
            topic("/b", "b_22222222", "Sensor"),
            topic("/a", "a_11111111", "foxglove.CompressedVideo"),
            topic("/b", "b_33333333", "Sensor2"),
        ];
        let catalog = StreamCatalog::new(topics, &episode());
        let labels = catalog
            .streams()
            .iter()
            .map(RecordedStream::label)
            .collect::<Vec<_>>();
        assert_eq!(labels, ["/a", "/b · 22222222", "/b · 33333333"]);
        assert_eq!(catalog.streams()[0].kind(), StreamKind::Video);
        assert_eq!(catalog.streams()[1].kind(), StreamKind::Data);
        assert_eq!(catalog.videos().count(), 1);
        assert!(catalog.get("b_33333333").is_some());
    }
}

#[cfg(test)]
mod camera_preference_tests {
    use super::StreamCatalog;

    #[test]
    fn the_leading_camera_looks_at_the_workspace_before_the_gripper() {
        let named = |names: [&str; 3]| names.map(str::to_owned).to_vec();
        assert_eq!(
            StreamCatalog::overview_camera(&named([
                "/left-wrist-camera",
                "/top-camera",
                "/right-wrist-camera"
            ])),
            Some("/top-camera")
        );
        assert_eq!(
            StreamCatalog::overview_camera(&named(["/cam_low", "/cam_right_wrist", "/HEAD_CAM"])),
            Some("/HEAD_CAM")
        );
        assert_eq!(
            StreamCatalog::overview_camera(&named(["/gripper_cam", "/scene", "/front_rgb"])),
            Some("/front_rgb")
        );
        assert_eq!(StreamCatalog::overview_camera(&[]), None);
    }
}
