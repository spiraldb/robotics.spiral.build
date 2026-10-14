//! A dataset's `episodes` table, one row per episode: its identity, where it
//! was read from, and the facts readers list it by, including the streams it
//! holds messages of ([datasets](../../../../../docs/datasets.md#episodes)).
use super::EpisodeUuids;
use super::writer::{DecodedEpisode, EpisodeOrigin};
use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{
    ArrayRef, Int64Array, ListArray, RecordBatch, StringArray, StructArray, UInt32Array,
    UInt64Array,
};
use arrow_buffer::OffsetBuffer;
use arrow_schema::{DataType, Field, Fields, Schema, SchemaRef};
use spiral_robotics_common::datasets::EpisodeId;
use std::sync::Arc;

pub(crate) struct EpisodeRows;

impl EpisodeRows {
    /// The table's columns.
    pub(crate) fn schema() -> SchemaRef {
        let tasks = DataType::List(Arc::new(Field::new_list_field(DataType::Utf8, false)));
        Arc::new(Schema::new(vec![
            EpisodeUuids::field("id"),
            Field::new("key", DataType::Utf8, false),
            Field::new("source_uri", DataType::Utf8, true),
            Field::new("source_bytes", DataType::UInt64, true),
            Field::new("sha256", DataType::Utf8, true),
            Field::new("episode_index", DataType::Int64, true),
            Field::new("tasks", tasks, true),
            Field::new("message_count", DataType::UInt64, false),
            Field::new("channel_count", DataType::UInt32, false),
            Field::new("streams", Self::streams_type(), false),
            Field::new("message_start_time", DataType::UInt64, false),
            Field::new("message_end_time", DataType::UInt64, false),
            Field::new("duration_ns", DataType::UInt64, false),
        ]))
    }

    /// Each stream an episode holds messages of, and how many.
    fn stream_fields() -> Fields {
        Fields::from(vec![
            Field::new("stream", DataType::Utf8, false),
            Field::new("messages", DataType::UInt64, false),
        ])
    }

    fn streams_type() -> DataType {
        DataType::List(Arc::new(Field::new_list_field(
            DataType::Struct(Self::stream_fields()),
            false,
        )))
    }

    /// The rows of `episodes`, each under the id it is written with.
    pub(crate) fn batch(episodes: &[(EpisodeId, DecodedEpisode)]) -> Result<RecordBatch, String> {
        let ids = episodes
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let episodes = episodes
            .iter()
            .map(|(_, episode)| episode)
            .collect::<Vec<_>>();
        let source_uris = episodes.iter().map(|episode| match episode.origin() {
            EpisodeOrigin::Mcap(file) => file.uri(),
            EpisodeOrigin::LeRobot(origin) => Some(origin.root()),
        });
        let mut tasks = ListBuilder::new(StringBuilder::new())
            .with_field(Field::new_list_field(DataType::Utf8, false));
        for episode in &episodes {
            match episode.origin().lerobot() {
                Some(origin) => {
                    for task in origin.tasks() {
                        tasks.values().append_value(task);
                    }
                    tasks.append(true);
                }
                None => tasks.append(false),
            }
        }
        let spans = episodes
            .iter()
            .map(|episode| episode.time_span())
            .collect::<Vec<_>>();
        let columns: Vec<ArrayRef> = vec![
            EpisodeUuids::array(&ids)?,
            Arc::new(StringArray::from_iter_values(
                episodes.iter().map(|episode| episode.key()),
            )),
            Arc::new(StringArray::from_iter(source_uris)),
            Arc::new(UInt64Array::from_iter(
                episodes
                    .iter()
                    .map(|episode| episode.origin().mcap().map(|file| file.size())),
            )),
            Arc::new(StringArray::from_iter(episodes.iter().map(|episode| {
                episode.origin().mcap().map(|file| file.sha256())
            }))),
            Arc::new(Int64Array::from_iter(episodes.iter().map(|episode| {
                episode.origin().lerobot().map(|origin| origin.index())
            }))),
            Arc::new(tasks.finish()),
            Arc::new(UInt64Array::from_iter_values(episodes.iter().map(
                |episode| {
                    episode
                        .streams()
                        .iter()
                        .map(|messages| messages.len() as u64)
                        .sum::<u64>()
                },
            ))),
            Arc::new(UInt32Array::from_iter_values(
                episodes
                    .iter()
                    .map(|episode| episode.streams().len() as u32),
            )),
            Self::streams(&episodes)?,
            Arc::new(UInt64Array::from_iter_values(
                spans.iter().map(|span| span.0),
            )),
            Arc::new(UInt64Array::from_iter_values(
                spans.iter().map(|span| span.1),
            )),
            Arc::new(UInt64Array::from_iter_values(
                spans.iter().map(|(start, end)| end - start),
            )),
        ];
        RecordBatch::try_new(Self::schema(), columns)
            .map_err(|error| format!("building episode rows: {error}"))
    }

    /// Each episode's `streams`: every stream it holds messages of, in name
    /// order, with how many.
    fn streams(episodes: &[&DecodedEpisode]) -> Result<ArrayRef, String> {
        let mut names = Vec::new();
        let mut counts = Vec::new();
        let mut lengths = Vec::with_capacity(episodes.len());
        for episode in episodes {
            let mut streams = episode
                .streams()
                .iter()
                .map(|messages| (messages.stream().name().stream(), messages.len() as u64))
                .collect::<Vec<_>>();
            streams.sort();
            lengths.push(streams.len());
            for (name, count) in streams {
                names.push(name);
                counts.push(count);
            }
        }
        let streams = StructArray::try_new(
            Self::stream_fields(),
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(UInt64Array::from(counts)),
            ],
            None,
        )
        .map_err(|error| error.to_string())?;
        let DataType::List(item) = Self::streams_type() else {
            unreachable!("streams are a list");
        };
        ListArray::try_new(
            item,
            OffsetBuffer::from_lengths(lengths),
            Arc::new(streams),
            None,
        )
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| error.to_string())
    }
}
