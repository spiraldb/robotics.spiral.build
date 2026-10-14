//! One recorded stream of a dataset — a topic in one schema and Arrow type —
//! and the messages of it one episode holds, as rows of the stream's
//! `messages` table ([datasets](../../../../../docs/datasets.md#messages)).
use super::EpisodeUuids;
use arrow_array::{ArrayRef, BooleanArray, RecordBatch, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Fields, Schema};
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::datasets::{EpisodeId, TopicRevision};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// What one stream records: its topic, the native schema and encoding its
/// messages were recorded with, and the Arrow type each decodes to. Its name
/// is derived from all four, so a topic recorded two ways is two streams.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordedStream {
    name: TopicRevision,
    topic: String,
    schema: Option<String>,
    encoding: String,
    value_type: DataType,
}

impl RecordedStream {
    /// How many hex digits of the identity's hash end a stream's name.
    const HASH_DIGITS: usize = 8;
    /// The longest topic part a name keeps, so `_` and the hash still fit a
    /// stream name's 96 bytes.
    const TOPIC_BYTES: usize = 96 - 1 - Self::HASH_DIGITS;
    /// The schema camera video is recorded with in MCAP.
    pub(crate) const COMPRESSED_VIDEO: &str = "foxglove.CompressedVideo";
    /// The encoding of a camera Studio encoded itself.
    pub(crate) const H264: &str = "h264";

    pub(crate) fn new(
        topic: &str,
        schema: Option<&str>,
        encoding: &str,
        value_type: DataType,
    ) -> Result<Self, String> {
        let identity = serde_json::to_string(&(
            topic,
            schema,
            ArrowTypeText::try_from(&value_type)?.to_string(),
        ))
        .map_err(|error| format!("naming the stream of {topic}: {error}"))?;
        let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
        let name = TopicRevision::new(format!(
            "{}_{}",
            Self::slug(topic),
            &digest[..Self::HASH_DIGITS]
        ))?;
        Ok(Self {
            name,
            topic: topic.to_owned(),
            schema: schema.map(str::to_owned),
            encoding: encoding.to_owned(),
            value_type,
        })
    }

    /// `topic` in lowercase letters, digits and single `_`s, as long as a
    /// name allows: `/studio/joints/leader` is `studio_joints_leader`.
    fn slug(topic: &str) -> String {
        let mut slug = String::new();
        for character in topic.chars() {
            if character.is_ascii_alphanumeric() {
                slug.push(character.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('_') {
                slug.push('_');
            }
        }
        slug.truncate(Self::TOPIC_BYTES);
        slug.trim_end_matches('_').to_owned()
    }

    pub(crate) fn name(&self) -> &TopicRevision {
        &self.name
    }
    pub(crate) fn topic(&self) -> &str {
        &self.topic
    }
    pub(crate) fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
    }
    pub(crate) fn encoding(&self) -> &str {
        &self.encoding
    }
    pub(crate) fn value_type(&self) -> &DataType {
        &self.value_type
    }
    /// Whether it carries camera video, whose rows say which access units a
    /// decoder can start from.
    pub(crate) fn is_video(&self) -> bool {
        self.schema() == Some(Self::COMPRESSED_VIDEO) || self.encoding == Self::H264
    }
}

/// Where each of a stream's messages sits: its log and publish times and its
/// sequence number, in the stream's order.
#[derive(Clone, Default)]
pub(crate) struct MessagePositions {
    log_times: Vec<u64>,
    publish_times: Vec<u64>,
    sequences: Vec<u32>,
}

impl MessagePositions {
    pub(crate) fn push(&mut self, log_time: u64, publish_time: u64, sequence: u32) {
        self.log_times.push(log_time);
        self.publish_times.push(publish_time);
        self.sequences.push(sequence);
    }
    pub(crate) fn len(&self) -> usize {
        self.log_times.len()
    }
}

/// One stream's messages of one episode, in the stream's order, each one's
/// place its ordinal.
pub(crate) struct StreamMessages {
    stream: RecordedStream,
    positions: MessagePositions,
    /// For camera video, whether each access unit holds an IDR picture.
    keyframes: Option<Vec<bool>>,
    values: ArrayRef,
}

impl StreamMessages {
    /// `values` of `stream`, one per position, none null. A video stream's
    /// messages say which are keyframes; no other stream's do.
    pub(crate) fn new(
        stream: RecordedStream,
        positions: MessagePositions,
        keyframes: Option<Vec<bool>>,
        values: ArrayRef,
    ) -> Result<Self, String> {
        let topic = stream.topic();
        if values.data_type() != stream.value_type() || values.null_count() > 0 {
            return Err(format!(
                "{topic}'s messages are not all {}",
                stream.value_type()
            ));
        }
        if values.len() != positions.len()
            || keyframes
                .as_ref()
                .is_some_and(|keyframes| keyframes.len() != positions.len())
        {
            return Err(format!("{topic} holds as many messages as positions"));
        }
        if keyframes.is_some() != stream.is_video() {
            return Err(format!("only video streams mark keyframes, not {topic}"));
        }
        Ok(Self {
            stream,
            positions,
            keyframes,
            values,
        })
    }

    pub(crate) fn stream(&self) -> &RecordedStream {
        &self.stream
    }
    pub(crate) fn len(&self) -> usize {
        self.positions.len()
    }
    /// Its earliest and latest log times; none without messages.
    pub(crate) fn time_span(&self) -> Option<(u64, u64)> {
        let times = &self.positions.log_times;
        Some((*times.iter().min()?, *times.iter().max()?))
    }

    /// The columns of a stream's `messages` table: the episode, the
    /// message's position, for video whether it is a keyframe, and the value.
    pub(crate) fn schema(stream: &RecordedStream) -> Schema {
        let mut fields = vec![
            EpisodeUuids::field("id"),
            Field::new("log_time", DataType::UInt64, false),
            Field::new("publish_time", DataType::UInt64, false),
            Field::new("sequence", DataType::UInt32, false),
            Field::new("ordinal", DataType::UInt64, false),
        ];
        if stream.is_video() {
            fields.push(Field::new("keyframe", DataType::Boolean, false));
        }
        fields.push(Field::new("value", stream.value_type().clone(), false));
        Schema::new(fields)
    }

    /// Its rows of the stream's `messages` table, as episode `episode`'s.
    pub(crate) fn batch(&self, episode: &EpisodeId) -> Result<RecordBatch, String> {
        let MessagePositions {
            log_times,
            publish_times,
            sequences,
        } = &self.positions;
        let mut columns: Vec<ArrayRef> = vec![
            EpisodeUuids::array(&vec![episode.clone(); self.len()])?,
            Arc::new(UInt64Array::from(log_times.clone())),
            Arc::new(UInt64Array::from(publish_times.clone())),
            Arc::new(UInt32Array::from(sequences.clone())),
            Arc::new(UInt64Array::from_iter_values(0..self.len() as u64)),
        ];
        if let Some(keyframes) = &self.keyframes {
            columns.push(Arc::new(BooleanArray::from(keyframes.clone())));
        }
        columns.push(self.values.clone());
        RecordBatch::try_new(Arc::new(Self::schema(&self.stream)), columns)
            .map_err(|error| format!("{}'s messages: {error}", self.stream.topic()))
    }
}

/// An Arrow type spelled as Arrow's own tools print it, each nested field's
/// name and nullability included: `struct<data: binary_view not null>`. It
/// names a stream's type when the stream is named, so it never changes.
pub(crate) struct ArrowTypeText(String);

impl TryFrom<&DataType> for ArrowTypeText {
    type Error = String;

    fn try_from(data_type: &DataType) -> Result<Self, Self::Error> {
        let field = |field: &Field| -> Result<String, String> {
            let nullability = if field.is_nullable() { "" } else { " not null" };
            Ok(format!(
                "{}: {}{nullability}",
                field.name(),
                Self::try_from(field.data_type())?
            ))
        };
        let fields = |fields: &Fields| -> Result<String, String> {
            Ok(fields
                .iter()
                .map(|item| field(item))
                .collect::<Result<Vec<_>, _>>()?
                .join(", "))
        };
        Ok(Self(match data_type {
            DataType::Boolean => "bool".into(),
            DataType::Int8 => "int8".into(),
            DataType::Int16 => "int16".into(),
            DataType::Int32 => "int32".into(),
            DataType::Int64 => "int64".into(),
            DataType::UInt8 => "uint8".into(),
            DataType::UInt16 => "uint16".into(),
            DataType::UInt32 => "uint32".into(),
            DataType::UInt64 => "uint64".into(),
            DataType::Float16 => "halffloat".into(),
            DataType::Float32 => "float".into(),
            DataType::Float64 => "double".into(),
            DataType::Utf8 => "string".into(),
            DataType::LargeUtf8 => "large_string".into(),
            DataType::Utf8View => "string_view".into(),
            DataType::Binary => "binary".into(),
            DataType::LargeBinary => "large_binary".into(),
            DataType::BinaryView => "binary_view".into(),
            DataType::List(item) => format!("list<{}>", field(item)?),
            DataType::LargeList(item) => format!("large_list<{}>", field(item)?),
            DataType::FixedSizeList(item, size) => {
                format!("fixed_size_list<{}>[{size}]", field(item)?)
            }
            DataType::Struct(members) => format!("struct<{}>", fields(members)?),
            other => return Err(format!("A stream of {other} values cannot be stored")),
        }))
    }
}

impl Display for ArrowTypeText {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::BinaryViewArray;

    #[test]
    fn a_stream_is_named_for_its_topic_and_hashed_with_its_schema_and_type() {
        let joints = RecordedStream::new(
            "/studio/joints/leader",
            Some("studio.recording.JointSample"),
            "protobuf",
            DataType::Float64,
        )
        .unwrap();
        let name = joints.name().stream();
        assert!(name.starts_with("studio_joints_leader_"), "{name}");
        assert_eq!(name.len(), "studio_joints_leader_".len() + 8);
        // The same topic in another schema or type is another stream.
        let other_type = RecordedStream::new(
            "/studio/joints/leader",
            Some("studio.recording.JointSample"),
            "protobuf",
            DataType::Float32,
        )
        .unwrap();
        let schemaless =
            RecordedStream::new("/studio/joints/leader", None, "protobuf", DataType::Float64)
                .unwrap();
        assert_ne!(joints.name(), other_type.name());
        assert_ne!(joints.name(), schemaless.name());
        assert_eq!(
            RecordedStream::new(
                "/studio/joints/leader",
                Some("studio.recording.JointSample"),
                "protobuf",
                DataType::Float64,
            )
            .unwrap()
            .name(),
            joints.name()
        );
        for (topic, slug) in [
            ("observation.images.up", "observation_images_up_"),
            ("top_camera-images-rgb", "top_camera_images_rgb_"),
            (
                "/camera/follower.camera.top/video",
                "camera_follower_camera_top_video_",
            ),
            ("/", "_"),
        ] {
            let stream = RecordedStream::new(topic, None, "parquet", DataType::Float32).unwrap();
            assert!(stream.name().stream().starts_with(slug), "{topic}");
            assert_eq!(stream.name().stream().len(), slug.len() + 8, "{topic}");
        }
        let long =
            RecordedStream::new(&"x/".repeat(200), None, "json", DataType::BinaryView).unwrap();
        assert_eq!(long.name().stream().len(), 96);
    }

    #[test]
    fn types_are_spelled_as_arrow_prints_them() {
        let item = Field::new_list_field(DataType::Float32, false);
        let value = DataType::Struct(Fields::from(vec![
            Field::new("data", DataType::FixedSizeList(Arc::new(item), 14), false),
            Field::new("note", DataType::Utf8View, true),
            Field::new(
                "values",
                DataType::List(Arc::new(Field::new_list_field(DataType::Int64, false))),
                false,
            ),
        ]));
        assert_eq!(
            ArrowTypeText::try_from(&value).unwrap().to_string(),
            "struct<data: fixed_size_list<item: float not null>[14] not null, note: string_view, values: list<item: int64 not null> not null>"
        );
        assert!(ArrowTypeText::try_from(&DataType::Date32).is_err());
    }

    #[test]
    fn rows_carry_their_episode_position_and_for_video_keyframes() {
        let camera = RecordedStream::new(
            "observation.images.up",
            None,
            RecordedStream::H264,
            DataType::BinaryView,
        )
        .unwrap();
        let mut positions = MessagePositions::default();
        positions.push(0, 0, 0);
        positions.push(33_333_333, 33_333_333, 1);
        let values: ArrayRef = Arc::new(BinaryViewArray::from_iter_values([b"a", b"b"]));
        assert!(
            StreamMessages::new(
                camera.clone(),
                MessagePositions::default(),
                None,
                values.clone()
            )
            .is_err()
        );
        let messages =
            StreamMessages::new(camera, positions, Some(vec![true, false]), values).unwrap();
        assert_eq!(messages.time_span(), Some((0, 33_333_333)));
        let episode = EpisodeId::try_from("01900000000070008000000000000001").unwrap();
        let batch = messages.batch(&episode).unwrap();
        assert_eq!(
            batch
                .schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            [
                "id",
                "log_time",
                "publish_time",
                "sequence",
                "ordinal",
                "keyframe",
                "value"
            ]
        );
        let ordinals = batch
            .column_by_name("ordinal")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap();
        assert_eq!(ordinals.values().to_vec(), [0, 1]);
        assert_eq!(
            EpisodeUuids::read(batch.column(0).as_ref()).unwrap(),
            [episode.clone(), episode]
        );
    }
}
