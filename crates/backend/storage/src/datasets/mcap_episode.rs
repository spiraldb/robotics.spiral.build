//! One MCAP file as one episode: every channel's messages decoded by their
//! encoding — Protobuf as Arrow structs, JSON as its bytes — and grouped
//! into the streams they record, each in file order.
mod protobuf;

use super::files::McapFile;
use super::streams::{MessagePositions, RecordedStream, StreamMessages};
use super::writer::{DecodedEpisode, EpisodeOrigin};
use arrow_array::{ArrayRef, BinaryViewArray};
use arrow_schema::DataType;
use prost_reflect::{DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor};
use protobuf::ProtobufValues;
use spiral_robotics_common::datasets::{H264AccessUnit, TopicRevision};
use std::collections::HashMap;
use std::sync::Arc;

impl DecodedEpisode {
    /// `bytes`, the MCAP file `file` keeps, as one episode.
    pub(crate) fn of_mcap(file: McapFile, bytes: &[u8]) -> Result<Self, String> {
        let streams =
            McapStreams::read(bytes).map_err(|error| format!("{}: {error}", file.key()))?;
        Ok(Self::new(
            file.key().to_owned(),
            EpisodeOrigin::Mcap(file),
            streams,
        ))
    }
}

/// The streams of one file as its messages are read.
#[derive(Default)]
struct McapStreams {
    streams: Vec<StreamReading>,
    by_channel: HashMap<u16, usize>,
    by_name: HashMap<TopicRevision, usize>,
}

impl McapStreams {
    fn read(bytes: &[u8]) -> Result<Vec<StreamMessages>, String> {
        let mut streams = Self::default();
        for message in mcap::MessageStream::new(bytes).map_err(|error| error.to_string())? {
            streams.push(message.map_err(|error| error.to_string())?)?;
        }
        streams
            .streams
            .into_iter()
            .map(StreamReading::finish)
            .collect()
    }

    fn push(&mut self, message: mcap::Message<'_>) -> Result<(), String> {
        let index = match self.by_channel.get(&message.channel.id) {
            Some(index) => *index,
            None => self.open(&message.channel)?,
        };
        self.streams[index].push(&message)
    }

    /// The stream `channel` records, which another channel may record too.
    fn open(&mut self, channel: &mcap::Channel<'_>) -> Result<usize, String> {
        let reading = StreamReading::of(channel)?;
        let index = match self.by_name.get(reading.stream.name()) {
            Some(&index) if self.streams[index].stream == reading.stream => index,
            Some(_) => {
                return Err(format!(
                    "channels of different streams share the name {}",
                    reading.stream.name().stream()
                ));
            }
            None => {
                self.by_name
                    .insert(reading.stream.name().clone(), self.streams.len());
                self.streams.push(reading);
                self.streams.len() - 1
            }
        };
        self.by_channel.insert(channel.id, index);
        Ok(index)
    }
}

/// One stream's messages so far, and how they decode.
struct StreamReading {
    stream: RecordedStream,
    positions: MessagePositions,
    payloads: Payloads,
}

enum Payloads {
    Protobuf {
        values: ProtobufValues,
        messages: Vec<DynamicMessage>,
    },
    Json(Vec<Vec<u8>>),
}

impl StreamReading {
    const PROTOBUF: &str = "protobuf";
    const JSON: &str = "json";

    /// How `channel`'s messages decode, refusing an encoding Studio cannot.
    fn of(channel: &mcap::Channel<'_>) -> Result<Self, String> {
        let topic = &channel.topic;
        let schema = channel.schema.as_deref();
        let (encoding, value_type, payloads) = match channel.message_encoding.as_str() {
            Self::PROTOBUF => {
                let schema = schema
                    .filter(|schema| schema.encoding == Self::PROTOBUF)
                    .ok_or_else(|| format!("{topic} has no Protobuf schema"))?;
                let pool = DescriptorPool::decode(schema.data.as_ref())
                    .map_err(|error| format!("{topic}'s schema {}: {error}", schema.name))?;
                let message = pool
                    .get_message_by_name(&schema.name)
                    .ok_or_else(|| format!("{topic}'s schema does not define {}", schema.name))?;
                let values = ProtobufValues::new(message)?;
                let value_type = values.value_type();
                (
                    Self::PROTOBUF,
                    value_type,
                    Payloads::Protobuf {
                        values,
                        messages: Vec::new(),
                    },
                )
            }
            Self::JSON => (Self::JSON, DataType::BinaryView, Payloads::Json(Vec::new())),
            other => {
                return Err(format!(
                    "{topic} is encoded as {other:?}; Studio decodes only Protobuf and JSON"
                ));
            }
        };
        let stream = RecordedStream::new(
            topic,
            schema.map(|schema| schema.name.as_str()),
            encoding,
            value_type,
        )?;
        if stream.is_video() && encoding != Self::PROTOBUF {
            return Err(format!(
                "{topic} records {} as {encoding}; Studio reads camera video only as Protobuf",
                RecordedStream::COMPRESSED_VIDEO
            ));
        }
        Ok(Self {
            stream,
            positions: MessagePositions::default(),
            payloads,
        })
    }

    fn push(&mut self, message: &mcap::Message<'_>) -> Result<(), String> {
        match &mut self.payloads {
            Payloads::Protobuf { values, messages } => messages.push(
                DynamicMessage::decode(values.descriptor().clone(), message.data.as_ref())
                    .map_err(|error| format!("decoding {}: {error}", message.channel.topic))?,
            ),
            Payloads::Json(payloads) => payloads.push(message.data.to_vec()),
        }
        self.positions
            .push(message.log_time, message.publish_time, message.sequence);
        Ok(())
    }

    fn finish(self) -> Result<StreamMessages, String> {
        let (values, keyframes): (ArrayRef, _) = match &self.payloads {
            Payloads::Protobuf { values, messages } => {
                let keyframes = self
                    .stream
                    .is_video()
                    .then(|| CompressedVideo::new(values.descriptor(), self.stream.topic()))
                    .transpose()?
                    .map(|video| video.keyframes(messages))
                    .transpose()?;
                (values.array(messages)?, keyframes)
            }
            Payloads::Json(payloads) => {
                (Arc::new(BinaryViewArray::from_iter_values(payloads)), None)
            }
        };
        StreamMessages::new(self.stream, self.positions, keyframes, values)
    }
}

/// A `foxglove.CompressedVideo` stream: each message one H.264 access unit
/// in `data`, its codec named by `format`.
struct CompressedVideo<'a> {
    topic: &'a str,
    data: FieldDescriptor,
    format: FieldDescriptor,
}

impl<'a> CompressedVideo<'a> {
    fn new(message: &MessageDescriptor, topic: &'a str) -> Result<Self, String> {
        let field = |name: &str, kind: Kind| {
            message
                .get_field_by_name(name)
                .filter(|field| field.kind() == kind && !field.is_list())
                .ok_or_else(|| format!("{topic}'s video messages have no {name}"))
        };
        Ok(Self {
            topic,
            data: field("data", Kind::Bytes)?,
            format: field("format", Kind::String)?,
        })
    }

    /// Whether each message's access unit holds an IDR picture. Studio
    /// reads H.264 alone, so video in any other codec is refused.
    fn keyframes(&self, messages: &[DynamicMessage]) -> Result<Vec<bool>, String> {
        messages
            .iter()
            .map(|message| {
                let format = message.get_field(&self.format);
                let format = format.as_str().unwrap_or_default();
                if format != "h264" {
                    return Err(format!(
                        "{} is {format:?} video; Studio reads camera video only as H.264",
                        self.topic
                    ));
                }
                let data = message.get_field(&self.data);
                Ok(H264AccessUnit::holds_idr(
                    data.as_bytes()
                        .map(|bytes| bytes.as_ref())
                        .unwrap_or_default(),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
