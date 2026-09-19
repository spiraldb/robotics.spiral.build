//! Recorded streams of the selected episode: identity, schema, density, and
//! what the operator has chosen to show.
//!
//! A stream is one topic/schema revision inside one MCAP member. Its kind is
//! decided from recorded schema names only; nothing here reads meaning into
//! topic or field names.
use super::{
    DatasetClient,
    episode_transport::EpisodeTimeRange,
    message_buffer::{MessageBuffer, MessageField},
};
use dioxus::prelude::*;
use dioxus_workbench::PanelId;
use futures_util::StreamExt as _;
use spiral_robotics_common::datasets::{
    DatasetTopic, EpisodeCoverageRequest, EpisodeSelection, EpisodeTopicRequest, MessageRead,
    MessageWindow, StreamCoverage, TopicCoverage, TopicCoverageRequest,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const COVERAGE_BUCKETS: u16 = 240;
pub(super) const MAX_PLOTTED_FIELDS_PER_STREAM: usize = 16;

const CONCURRENT_SCHEMA_READS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StreamKind {
    Video,
    Data,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct RecordedStream {
    key: String,
    label: String,
    topic: DatasetTopic,
    read: MessageRead,
}

impl RecordedStream {
    fn new(topic: DatasetTopic, episode: &EpisodeSelection, duplicated: bool) -> Option<Self> {
        let revision = topic.revision()?.clone();
        let key = format!("{}#{}", topic.name(), revision.variant());
        let label = if duplicated {
            format!(
                "{} · {}",
                topic.name(),
                revision.variant().chars().take(8).collect::<String>()
            )
        } else {
            topic.name().to_owned()
        };
        let read = MessageRead::schema(
            EpisodeTopicRequest::new(
                episode.dataset().clone(),
                topic.name().to_owned(),
                episode.episode_id().to_owned(),
            ),
            revision,
        );
        Some(Self {
            key,
            label,
            topic,
            read,
        })
    }
    /// Stable within the episode; distinct schema variants of one topic differ.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
    pub(super) fn name(&self) -> &str {
        self.topic.name()
    }
    pub(super) fn kind(&self) -> StreamKind {
        if self
            .topic
            .schemas()
            .iter()
            .any(|schema| schema == "foxglove.CompressedVideo")
        {
            StreamKind::Video
        } else {
            StreamKind::Data
        }
    }
    pub(super) fn schemas(&self) -> &[String] {
        self.topic.schemas()
    }
    pub(super) fn samples(&self) -> Option<u64> {
        self.topic.samples()
    }
    pub(crate) fn read(&self) -> &MessageRead {
        &self.read
    }
    pub(super) fn panel_id(&self) -> PanelId {
        PanelId::new(format!("camera:{}", self.key))
    }
}

/// Every readable stream of one episode, in topic order.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct StreamCatalog {
    streams: Vec<RecordedStream>,
}

impl StreamCatalog {
    pub(crate) fn new(mut topics: Vec<DatasetTopic>, episode: &EpisodeSelection) -> Self {
        topics.sort_by(|left, right| {
            left.name().cmp(right.name()).then_with(|| {
                left.revision()
                    .map(|revision| revision.variant())
                    .cmp(&right.revision().map(|revision| revision.variant()))
            })
        });
        let mut names = BTreeMap::<String, usize>::new();
        for topic in &topics {
            *names.entry(topic.name().to_owned()).or_default() += 1;
        }
        let streams = topics
            .into_iter()
            .filter_map(|topic| {
                let duplicated = names.get(topic.name()).is_some_and(|count| *count > 1);
                RecordedStream::new(topic, episode, duplicated)
            })
            .collect();
        Self { streams }
    }
    pub(super) fn streams(&self) -> &[RecordedStream] {
        &self.streams
    }
    pub(crate) fn videos(&self) -> impl Iterator<Item = &RecordedStream> {
        self.streams
            .iter()
            .filter(|stream| stream.kind() == StreamKind::Video)
    }
    /// The part of a camera's name that tells it from the episode's other
    /// cameras: `/left-wrist-camera` beside `/top-camera` reads as
    /// "left-wrist" and "top". A lone camera keeps its whole name.
    pub(super) fn camera_title(&self, camera: &RecordedStream) -> String {
        let names = self.videos().map(RecordedStream::name).collect::<Vec<_>>();
        let name = camera.name();
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
    pub(super) fn data_by_traffic(&self) -> Vec<RecordedStream> {
        let mut ranked = self
            .streams
            .iter()
            .filter(|stream| stream.kind() == StreamKind::Data)
            .cloned()
            .collect::<Vec<_>>();
        ranked.sort_by_key(|stream| std::cmp::Reverse(stream.samples().unwrap_or(0)));
        ranked
    }
    pub(super) fn get(&self, key: &str) -> Option<&RecordedStream> {
        self.streams.iter().find(|stream| stream.key() == key)
    }
    pub(super) fn is_empty(&self) -> bool {
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

/// Discovery of the open episode's streams, as the panels see it.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum EpisodeStreams {
    Loading,
    Ready(StreamCatalog),
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub(super) enum StreamSchema {
    Loading,
    Ready(MessageField),
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub(super) enum CoverageState {
    Loading,
    Ready(TopicCoverage),
    Failed(String),
}

/// Numeric leaves, in schema order; lists count once and plot every index.
pub(super) trait PlottableLeaves {
    fn plottable_leaves(&self) -> Vec<MessageField>;
    /// What ticking a whole stream draws: measurements first. Floating-point
    /// and array fields are what a recording is about; integer and boolean
    /// scalars (counters, stamps, flags) stay one click away unless the
    /// stream has nothing else. This is a type rule, not a name rule.
    fn default_plot_leaves(&self) -> Vec<MessageField>;
}

impl PlottableLeaves for MessageField {
    fn default_plot_leaves(&self) -> Vec<MessageField> {
        let leaves = self.plottable_leaves();
        let mut preferred = leaves
            .iter()
            .filter(|leaf| leaf.is_floating() || leaf.is_list_item())
            .cloned()
            .collect::<Vec<_>>();
        if preferred.is_empty() {
            preferred = leaves;
        }
        preferred.truncate(MAX_PLOTTED_FIELDS_PER_STREAM);
        preferred
    }
    fn plottable_leaves(&self) -> Vec<MessageField> {
        let children = self.children();
        if children.is_empty() {
            return if self.plottable() {
                vec![self.clone()]
            } else {
                vec![]
            };
        }
        children
            .iter()
            .flat_map(PlottableLeaves::plottable_leaves)
            .collect()
    }
}

/// Which fields of which streams are drawn, and which traces are hidden.
#[derive(Clone, PartialEq, Default)]
struct PlotSelection {
    fields: BTreeMap<String, Vec<MessageField>>,
    hidden_traces: BTreeSet<String>,
    requested: BTreeMap<String, bool>,
}

impl PlotSelection {
    /// The plot an episode opens with when the operator has chosen nothing.
    /// Never applied after the operator has made a plot choice, and never
    /// for a stream without numeric fields. Reports whether it was adopted.
    fn adopt_default(&mut self, key: &str, fields: Vec<MessageField>) -> bool {
        if !self.requested.is_empty() || fields.is_empty() {
            return false;
        }
        self.fields.insert(key.to_owned(), fields);
        true
    }

    fn trace_key(stream: &str, trace: &str) -> String {
        format!("{stream}\u{1f}{trace}")
    }
}

/// Whether none, some, or every plottable field of a stream is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PlotState {
    None,
    Some,
    All,
}

/// Everything the operator has chosen to show for the current episode.
///
/// Reads spawned here belong to the workspace scope, so closing a panel does
/// not abandon a schema or coverage load another panel is waiting on.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct StreamChoices {
    scope: ScopeId,
    generation: Signal<u64>,
    plots: Signal<PlotSelection>,
    schemas: Signal<BTreeMap<String, StreamSchema>>,
    coverage: Signal<BTreeMap<String, CoverageState>>,
    selected: Signal<Option<String>>,
    cameras: Signal<BTreeSet<String>>,
}

impl StreamChoices {
    pub(super) fn use_choices() -> Self {
        use_hook(|| Self {
            scope: dioxus::dioxus_core::current_scope_id(),
            generation: Signal::new(0),
            plots: Signal::new(PlotSelection::default()),
            schemas: Signal::new(BTreeMap::new()),
            coverage: Signal::new(BTreeMap::new()),
            selected: Signal::new(None),
            cameras: Signal::new(BTreeSet::new()),
        })
    }

    /// Forget the previous episode; every camera starts visible.
    pub(super) fn reset(&mut self, catalog: &StreamCatalog) {
        let generation = self.generation.peek().wrapping_add(1);
        self.generation.set(generation);
        self.plots.set(PlotSelection::default());
        self.schemas.set(BTreeMap::new());
        self.coverage.set(BTreeMap::new());
        self.selected.set(None);
        self.cameras.set(
            catalog
                .videos()
                .map(|stream| stream.key().to_owned())
                .collect(),
        );
    }

    fn spawn<F: Future<Output = ()> + 'static>(&self, future: F) {
        dioxus::dioxus_core::Runtime::current().spawn(self.scope, future);
    }

    pub(super) fn plotted_fields(&self, key: &str) -> Vec<MessageField> {
        self.plots
            .read()
            .fields
            .get(key)
            .cloned()
            .unwrap_or_default()
    }
    /// Streams with at least one drawn field, in topic order.
    pub(super) fn plotted_streams(&self) -> Vec<String> {
        self.plots
            .read()
            .fields
            .iter()
            .filter(|(_, fields)| !fields.is_empty())
            .map(|(key, _)| key.clone())
            .collect()
    }
    pub(super) fn is_field_plotted(&self, key: &str, field: &MessageField) -> bool {
        self.plots
            .read()
            .fields
            .get(key)
            .is_some_and(|fields| fields.contains(field))
    }
    pub(super) fn toggle_field(&mut self, key: &str, field: MessageField) {
        let mut plots = self.plots.write();
        plots.requested.insert(key.to_owned(), false);
        let fields = plots.fields.entry(key.to_owned()).or_default();
        if let Some(index) = fields.iter().position(|existing| existing == &field) {
            fields.remove(index);
        } else if fields.len() < MAX_PLOTTED_FIELDS_PER_STREAM {
            fields.push(field);
        }
    }
    pub(super) fn plot_state(&self, key: &str) -> PlotState {
        let plotted = self.plotted_fields(key);
        if plotted.is_empty() {
            return PlotState::None;
        }
        let Some(StreamSchema::Ready(root)) = self.schema(key) else {
            return PlotState::Some;
        };
        let default = root.default_plot_leaves();
        if !default.is_empty() && default.iter().all(|leaf| plotted.contains(leaf)) {
            PlotState::All
        } else {
            PlotState::Some
        }
    }
    /// Draw every numeric field of a stream, loading its schema first if needed.
    pub(super) fn set_stream_plotted(&mut self, stream: &RecordedStream, plotted: bool) {
        let key = stream.key().to_owned();
        self.plots.write().requested.insert(key.clone(), plotted);
        if !plotted {
            self.plots.write().fields.remove(&key);
            return;
        }
        let mut choices = *self;
        let generation = *self.generation.peek();
        let read = stream.read().clone();
        self.spawn(async move {
            let Some(root) = choices.schema_for(&key, read).await else {
                return;
            };
            if *choices.generation.peek() != generation {
                return;
            }
            let mut plots = choices.plots.write();
            if plots.requested.get(&key) == Some(&true) {
                plots.fields.insert(key, root.default_plot_leaves());
            }
        });
    }
    /// Called once by Data when an episode opens: reads every data stream's
    /// schema, busiest first and a few at a time, and plots the first one
    /// with numeric fields, so Plots is never empty on open. The remaining
    /// schemas keep loading for the fold and the disabled checks. Other
    /// episode compositions can discover cameras without any of this.
    pub(super) fn plot_default_stream(&mut self, catalog: &StreamCatalog) {
        let ranked = catalog.data_by_traffic();
        let generation = *self.generation.peek();
        let mut choices = *self;
        self.spawn(async move {
            let mut answers = futures_util::stream::iter(ranked)
                .map(move |stream| {
                    let mut choices = choices;
                    async move {
                        let root = choices
                            .schema_for(stream.key(), stream.read().clone())
                            .await;
                        (stream, root)
                    }
                })
                .buffered(CONCURRENT_SCHEMA_READS);
            let mut chosen = false;
            while let Some((stream, root)) = answers.next().await {
                if *choices.generation.peek() != generation {
                    return;
                }
                if chosen {
                    continue;
                }
                if let Some(root) = root {
                    chosen = choices
                        .plots
                        .write()
                        .adopt_default(stream.key(), root.default_plot_leaves());
                }
            }
        });
    }
    /// A stream with nothing to plot: at most one message, or a schema known
    /// to have no numeric fields. Cameras are never quiet. This is a count
    /// rule and a type rule, never a name rule.
    pub(super) fn stream_is_quiet(&self, stream: &RecordedStream) -> bool {
        if stream.kind() == StreamKind::Video {
            return false;
        }
        stream.samples().is_some_and(|samples| samples <= 1)
            || matches!(
                self.schemas.read().get(stream.key()),
                Some(StreamSchema::Ready(root)) if root.plottable_leaves().is_empty()
            )
    }

    pub(super) fn trace_hidden(&self, key: &str, trace: &str) -> bool {
        self.plots
            .read()
            .hidden_traces
            .contains(&PlotSelection::trace_key(key, trace))
    }
    pub(super) fn toggle_trace(&mut self, key: &str, trace: &str) {
        let trace_key = PlotSelection::trace_key(key, trace);
        let mut plots = self.plots.write();
        plots.requested.insert(key.to_owned(), false);
        if !plots.hidden_traces.remove(&trace_key) {
            plots.hidden_traces.insert(trace_key);
        }
    }

    pub(super) fn schema(&self, key: &str) -> Option<StreamSchema> {
        self.schemas.read().get(key).cloned()
    }
    /// Start reading a stream's schema once; later calls observe the result.
    pub(super) fn ensure_schema(&mut self, stream: &RecordedStream) {
        if self.schemas.peek().contains_key(stream.key()) {
            return;
        }
        let key = stream.key().to_owned();
        let read = stream.read().clone();
        let mut choices = *self;
        self.spawn(async move {
            choices.schema_for(&key, read).await;
        });
    }
    async fn schema_for(&mut self, key: &str, read: MessageRead) -> Option<MessageField> {
        if let Some(StreamSchema::Ready(root)) = self.schemas.peek().get(key) {
            return Some(root.clone());
        }
        let generation = *self.generation.peek();
        self.schemas
            .write()
            .insert(key.to_owned(), StreamSchema::Loading);
        let result = match MessageBuffer::load(read, false).await {
            Ok(buffer) => buffer.value_field(),
            Err(error) => Err(error),
        };
        if *self.generation.peek() != generation {
            return None;
        }
        let state = match &result {
            Ok(root) => StreamSchema::Ready(root.clone()),
            Err(error) => StreamSchema::Failed(error.clone()),
        };
        self.schemas.write().insert(key.to_owned(), state);
        result.ok()
    }

    pub(super) fn coverage(&self, key: &str) -> Option<CoverageState> {
        self.coverage.read().get(key).cloned()
    }
    /// Count every stream's messages into timeline buckets with one request,
    /// so the rows fill together instead of one at a time.
    pub(super) fn load_coverage(&mut self, catalog: &StreamCatalog, range: EpisodeTimeRange) {
        let generation = *self.generation.peek();
        let (keys, streams): (Vec<String>, Vec<TopicCoverageRequest>) = catalog
            .streams()
            .iter()
            .filter(|stream| stream.samples() != Some(0))
            .map(|stream| {
                (
                    stream.key().to_owned(),
                    TopicCoverageRequest::new(
                        stream
                            .read()
                            .clone()
                            .with_window(MessageWindow::new(range.bounds(), vec![])),
                        COVERAGE_BUCKETS,
                    ),
                )
            })
            .unzip();
        if keys.is_empty() {
            return;
        }
        {
            let mut coverage = self.coverage.write();
            for key in &keys {
                coverage.insert(key.clone(), CoverageState::Loading);
            }
        }
        let mut choices = *self;
        self.spawn(async move {
            let answer = DatasetClient::current()
                .episode_coverage(EpisodeCoverageRequest::new(streams))
                .await;
            if *choices.generation.peek() != generation {
                return;
            }
            let outcomes = match answer {
                Ok(outcomes) if outcomes.len() == keys.len() => outcomes,
                Ok(outcomes) => {
                    let error = format!(
                        "Coverage answered for {} of {} streams",
                        outcomes.len(),
                        keys.len()
                    );
                    vec![StreamCoverage::Failed { error }; keys.len()]
                }
                Err(error) => vec![
                    StreamCoverage::Failed {
                        error: error.to_string()
                    };
                    keys.len()
                ],
            };
            let mut coverage = choices.coverage.write();
            for (key, outcome) in keys.into_iter().zip(outcomes) {
                let state = match outcome {
                    StreamCoverage::Ready { coverage } => CoverageState::Ready(coverage),
                    StreamCoverage::Failed { error } => CoverageState::Failed(error),
                };
                coverage.insert(key, state);
            }
        });
    }

    pub(super) fn selected(&self) -> Option<String> {
        self.selected.read().clone()
    }
    pub(super) fn select(&mut self, key: Option<String>) {
        self.selected.set(key);
    }

    pub(super) fn camera_open(&self, key: &str) -> bool {
        self.cameras.read().contains(key)
    }
    pub(super) fn set_camera(&mut self, key: &str, open: bool) {
        let mut cameras = self.cameras.write();
        if open {
            cameras.insert(key.to_owned());
        } else {
            cameras.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Fields};
    use spiral_robotics_common::datasets::{DatasetSelection, TopicRevision};

    fn episode() -> EpisodeSelection {
        EpisodeSelection::new(
            DatasetSelection::new(vec!["dataset_default".into()]).unwrap(),
            "01a0989fb65870a7a5459902f9e3e090".into(),
        )
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn the_default_plot_is_adopted_once_and_never_over_an_operator_choice() {
        let numeric = MessageField::root(&Field::new("value", DataType::Float64, false));
        let text = MessageField::root(&Field::new("value", DataType::Utf8, false));
        let mut selection = PlotSelection::default();
        // A stream without numeric fields is never the default, however busy.
        assert!(!selection.adopt_default("text", text.default_plot_leaves()));
        assert!(selection.adopt_default("dense", numeric.default_plot_leaves()));
        assert_eq!(
            selection
                .fields
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["dense"]
        );
        // An explicit uncheck wins even if the automatic schema read completes later.
        selection.requested.insert("dense".into(), false);
        selection.fields.clear();
        assert!(!selection.adopt_default("dense", numeric.default_plot_leaves()));
        assert!(selection.fields.is_empty());
        // A fresh episode gets a fresh default; the previous user's choice is not global.
        let mut next_episode = PlotSelection::default();
        assert!(next_episode.adopt_default("dense", numeric.default_plot_leaves()));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn camera_titles_keep_only_what_differs_between_the_episode_cameras() {
        let titles = |names: &[&str]| {
            let topics = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    DatasetTopic::new((*name).into(), vec!["foxglove.CompressedVideo".into()])
                        .with_revision(TopicRevision::new("v".into(), format!("{index:09}")))
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
            DatasetTopic::new("/a".into(), vec!["Sensor".into()])
                .with_revision(TopicRevision::new("v".into(), "111111111".into()))
                .with_coverage(1, Some(600)),
            DatasetTopic::new("/b".into(), vec!["Sensor".into()])
                .with_revision(TopicRevision::new("v".into(), "222222222".into()))
                .with_coverage(1, Some(9_000)),
            DatasetTopic::new("/c".into(), vec!["Sensor".into()])
                .with_revision(TopicRevision::new("v".into(), "333333333".into()))
                .with_coverage(1, Some(600)),
            DatasetTopic::new("/cam".into(), vec!["foxglove.CompressedVideo".into()])
                .with_revision(TopicRevision::new("v".into(), "444444444".into()))
                .with_coverage(1, Some(90_000)),
        ];
        let ranked = StreamCatalog::new(topics, &episode())
            .data_by_traffic()
            .iter()
            .map(|stream| stream.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ranked, ["/b", "/a", "/c"]);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn streams_are_keyed_by_revision_and_typed_by_schema_name() {
        let topics = vec![
            DatasetTopic::new("/b".into(), vec!["Sensor".into()])
                .with_revision(TopicRevision::new("v".into(), "222222222".into())),
            DatasetTopic::new("/a".into(), vec!["foxglove.CompressedVideo".into()])
                .with_revision(TopicRevision::new("v".into(), "111111111".into())),
            DatasetTopic::new("/b".into(), vec!["Sensor2".into()])
                .with_revision(TopicRevision::new("v".into(), "333333333".into())),
            DatasetTopic::new("/unreadable".into(), vec![]),
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
        assert_eq!(
            catalog.streams()[0].panel_id().as_str(),
            "camera:/a#111111111"
        );
        assert!(catalog.get("/b#333333333").is_some());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn plottable_leaves_walk_structs_and_count_lists_once() {
        let root = Field::new(
            "value",
            DataType::Struct(Fields::from(vec![
                Field::new(
                    "timestamp",
                    DataType::Struct(Fields::from(vec![
                        Field::new("seconds", DataType::Int64, false),
                        Field::new("nanos", DataType::Int32, false),
                    ])),
                    false,
                ),
                Field::new(
                    "position",
                    DataType::List(std::sync::Arc::new(Field::new(
                        "item",
                        DataType::Float64,
                        false,
                    ))),
                    false,
                ),
                Field::new("label", DataType::Utf8, true),
            ])),
            false,
        );
        let field = MessageField::root(&root);
        let labels = field
            .plottable_leaves()
            .iter()
            .map(MessageField::label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["timestamp.seconds", "timestamp.nanos", "position[]"]
        );
        let defaults = field
            .default_plot_leaves()
            .iter()
            .map(MessageField::label)
            .collect::<Vec<_>>();
        assert_eq!(defaults, ["position[]"]);
        let counters = MessageField::root(&Field::new(
            "value",
            DataType::Struct(Fields::from(vec![Field::new(
                "count",
                DataType::UInt32,
                false,
            )])),
            false,
        ));
        assert_eq!(counters.default_plot_leaves().len(), 1);
    }
}
