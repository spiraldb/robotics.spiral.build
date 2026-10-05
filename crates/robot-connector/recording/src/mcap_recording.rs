//! One self-contained MCAP recording: joint messages, metadata, and H.264 access units.
use super::capture::FullEpisode;
use super::mcap_schema::{McapMessage, RecordingSchema};
use base64::Engine as _;
use mcap::records::MessageHeader;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Value};
use spiral_robotics_common::datasets::{EpisodeVideo, H264AccessUnit, VideoPacket};
use std::collections::BTreeMap;
use std::io::Cursor;

/// Timestamped camera access units, before they are multiplexed into the MCAP file.
#[derive(Clone, Debug)]
pub struct CameraRecording {
    stream: String,
    camera_id: String,
    dimensions: (u32, u32, u32),
    frames: Vec<CameraAccessUnit>,
}

#[derive(Clone, Debug)]
struct CameraAccessUnit {
    elapsed_ns: u64,
    data: Vec<u8>,
}
impl CameraRecording {
    pub fn preview(
        cameras: &[Self],
        stream: &str,
    ) -> Result<spiral_robotics_common::datasets::EpisodeVideo, String> {
        let mut packets = CameraPackets::default();
        for camera in cameras.iter().filter(|camera| camera.stream == stream) {
            for frame in &camera.frames {
                packets.push(frame.elapsed_ns, &frame.data)?;
            }
        }
        packets.finish()
    }

    pub fn from_annex_b(
        stream: String,
        dimensions: (u32, u32, u32),
        encoded: (Vec<u8>, Vec<u64>),
    ) -> Result<Self, String> {
        let (bytes, times) = encoded;
        let mut boundaries = Vec::new();
        let mut index = 0;
        while index + 3 < bytes.len() {
            let prefix = if bytes[index..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if bytes[index..].starts_with(&[0, 0, 1]) {
                3
            } else {
                index += 1;
                continue;
            };
            if index + prefix < bytes.len() && bytes[index + prefix] & 31 == 9 {
                boundaries.push(index);
            }
            index += prefix;
        }
        if boundaries.len() != times.len() || times.is_empty() {
            return Err(format!(
                "Camera {stream} encoded {} access units for {} timestamps",
                boundaries.len(),
                times.len()
            ));
        }
        if !times.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("Camera timestamps must increase strictly".into());
        }
        boundaries[0] = 0;
        boundaries.push(bytes.len());
        let frames = times
            .into_iter()
            .zip(boundaries.windows(2))
            .map(|(elapsed_ns, range)| CameraAccessUnit {
                elapsed_ns,
                data: bytes[range[0]..range[1]].to_vec(),
            })
            .collect();
        Ok(Self {
            camera_id: stream.clone(),
            stream,
            dimensions,
            frames,
        })
    }
    pub fn with_camera_id(mut self, camera_id: String) -> Self {
        self.camera_id = camera_id;
        self
    }
    pub fn encoded_bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.data.len()).sum()
    }

    /// The camera's video `Stream` message. Each is written alone, so every
    /// camera stream carries ordinal 0.
    fn stream_message(&self, episode_id: &str, schema: &RecordingSchema) -> McapMessage {
        let (width, height, fps) = self.dimensions;
        let mut message = schema.message("Stream", format!("/studio/streams/{}", self.stream));
        message
            .set("episode_id", Value::String(episode_id.to_owned()))
            .set("stream_id", Value::String(self.stream.clone()))
            .set("ordinal", Value::U32(0))
            .set("modality", Value::String("video".to_owned()))
            .set("camera_id", Value::String(self.camera_id.clone()))
            .set("width", Value::U32(width))
            .set("height", Value::U32(height))
            .set("fps", Value::U32(fps));
        message
    }
}

pub struct McapRecording {
    writer: mcap::Writer<Cursor<Vec<u8>>>,
    origin_ns: u64,
}
impl McapRecording {
    pub fn encode(episode: &FullEpisode, cameras: &[CameraRecording]) -> Result<Vec<u8>, String> {
        let origin_ns = u64::try_from(episode.start().started_ms())
            .map_err(|_| "Recording time predates Unix epoch")?
            .checked_mul(1_000_000)
            .ok_or("Recording origin overflows nanoseconds")?;
        let mut recording = Self {
            writer: mcap::Writer::new(Cursor::new(Vec::new())).map_err(|e| e.to_string())?,
            origin_ns,
        };
        let schema = RecordingSchema::load()?;
        let start = episode.start();
        recording.write(
            &episode
                .episode_message(&schema)
                .map_err(|e| e.to_string())?,
        )?;
        for message in start.joint_stream_messages(episode.completion(), &schema) {
            recording.write(&message)?;
        }
        for (index, sample) in (0_u64..).zip(episode.samples()) {
            for message in start
                .joint_sample_messages(sample, index, &schema)
                .map_err(|e| e.to_string())?
            {
                recording.write(&message)?;
            }
        }
        let mut streams = std::collections::BTreeSet::new();
        for camera in cameras {
            if streams.insert(camera.stream.clone()) {
                recording.write(&camera.stream_message(start.id(), &schema))?;
            }
            recording.camera(camera)?;
        }
        recording.writer.finish().map_err(|e| e.to_string())?;
        Ok(recording.writer.into_inner().into_inner())
    }

    fn write(&mut self, message: &McapMessage) -> Result<(), String> {
        let schema_id = self
            .writer
            .add_schema(&message.schema_name(), "protobuf", RecordingSchema::BYTES)
            .map_err(|e| e.to_string())?;
        let channel_id = self
            .writer
            .add_channel(schema_id, message.topic(), "protobuf", &BTreeMap::new())
            .map_err(|e| e.to_string())?;
        let timestamp = self
            .origin_ns
            .checked_add(message.elapsed_ns())
            .ok_or("Recording timestamp overflow")?;
        let header = MessageHeader {
            channel_id,
            sequence: message.sequence(),
            log_time: timestamp,
            publish_time: timestamp,
        };
        self.writer
            .write_to_known_channel(&header, &message.encode())
            .map_err(|e| e.to_string())
    }
    fn camera(&mut self, camera: &CameraRecording) -> Result<(), String> {
        let schema = include_bytes!("schemas/compressed-video.bin");
        let pool = DescriptorPool::decode(schema.as_slice()).map_err(|e| e.to_string())?;
        let descriptor = pool
            .get_message_by_name("foxglove.CompressedVideo")
            .ok_or("CompressedVideo schema is missing")?;
        let clock = pool
            .get_message_by_name("google.protobuf.Timestamp")
            .ok_or("Timestamp schema is missing")?;
        let schema_id = self
            .writer
            .add_schema("foxglove.CompressedVideo", "protobuf", schema)
            .map_err(|e| e.to_string())?;
        let metadata = BTreeMap::from([
            ("width".into(), camera.dimensions.0.to_string()),
            ("height".into(), camera.dimensions.1.to_string()),
        ]);
        let channel_id = self
            .writer
            .add_channel(
                schema_id,
                &format!("/camera/{}/video", camera.stream),
                "protobuf",
                &metadata,
            )
            .map_err(|e| e.to_string())?;
        for (sequence, frame) in camera.frames.iter().enumerate() {
            let timestamp = self
                .origin_ns
                .checked_add(frame.elapsed_ns)
                .ok_or("Camera timestamp overflow")?;
            let mut time = DynamicMessage::new(clock.clone());
            time.set_field_by_name("seconds", Value::I64((timestamp / 1_000_000_000) as i64));
            time.set_field_by_name("nanos", Value::I32((timestamp % 1_000_000_000) as i32));
            let mut message = DynamicMessage::new(descriptor.clone());
            message.set_field_by_name("timestamp", Value::Message(time));
            message.set_field_by_name("frame_id", Value::String(camera.stream.clone()));
            message.set_field_by_name("format", Value::String("h264".into()));
            message.set_field_by_name("data", Value::Bytes(frame.data.clone().into()));
            let header = MessageHeader {
                channel_id,
                sequence: u32::try_from(sequence).map_err(|_| "Camera sequence overflow")?,
                log_time: timestamp,
                publish_time: timestamp,
            };
            self.writer
                .write_to_known_channel(&header, &message.encode_to_vec())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

const MAX_PACKETS: usize = 18_000;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct CameraPackets {
    packets: Vec<VideoPacket>,
    codec: Option<String>,
    previous_time: Option<u64>,
    encoded_bytes: usize,
}
impl CameraPackets {
    fn push(&mut self, timestamp: u64, bytes: &[u8]) -> Result<(), String> {
        if self.packets.len() >= MAX_PACKETS {
            return Err("Camera exceeds the 18,000-packet playback limit".into());
        }
        if self
            .previous_time
            .is_some_and(|previous| timestamp <= previous)
        {
            return Err("Camera timestamps must increase strictly for playback".into());
        }
        self.encoded_bytes = self.encoded_bytes.saturating_add(bytes.len());
        if self.encoded_bytes > MAX_ENCODED_BYTES {
            return Err("Camera exceeds the 64 MiB encoded playback limit".into());
        }
        let access = H264AccessUnit::parse(bytes)?;
        if self.packets.is_empty() && !access.keyframe() {
            return Err("Camera does not begin with a decodable H.264 keyframe".into());
        }
        if let Some(codec) = access.codec().map(str::to_owned) {
            if self
                .codec
                .as_ref()
                .is_some_and(|previous| previous != &codec)
            {
                return Err("Camera changes H.264 codec configuration during this episode".into());
            }
            self.codec = Some(codec);
        }
        self.packets.push(VideoPacket::new(
            timestamp,
            base64::engine::general_purpose::STANDARD.encode(bytes),
            access.keyframe(),
        ));
        self.previous_time = Some(timestamp);
        Ok(())
    }

    fn finish(self) -> Result<EpisodeVideo, String> {
        if self.packets.is_empty() {
            return Err("This camera has no video in the selected episode".into());
        }
        Ok(EpisodeVideo::new(
            self.codec
                .ok_or("Camera has no H.264 sequence parameter set")?,
            self.packets,
        ))
    }
}

#[cfg(test)]
mod camera_packet_tests {
    use super::*;
    #[test]
    fn annex_b_random_access_requires_idr_and_parameter_sets() {
        let packet = [
            0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let access = H264AccessUnit::parse(&packet).unwrap();
        assert!(access.keyframe());
        assert_eq!(access.codec(), Some("avc1.64001e"));
        assert!(
            !H264AccessUnit::parse(&[0, 0, 1, 0x41, 0x88])
                .unwrap()
                .keyframe()
        );
        assert!(H264AccessUnit::parse(&[0, 0, 1, 0x65, 0x88]).is_err());
        assert!(H264AccessUnit::parse(&[0, 0, 0, 5, 0x65]).is_err());
        assert!(H264AccessUnit::parse(&[]).is_err());
    }
    #[test]
    fn camera_reads_reject_empty_and_unordered_streams() {
        assert!(CameraPackets::default().finish().is_err());
        let key: &[u8] = &[
            0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let mut packets = CameraPackets::default();
        packets.push(10, key).unwrap();
        assert!(
            packets
                .push(10, key)
                .unwrap_err()
                .contains("increase strictly")
        );
    }
}
