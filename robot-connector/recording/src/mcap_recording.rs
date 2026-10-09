//! One episode's MCAP recording, written while the episode records: joint samples
//! and camera access units in the order they arrive, then the messages that
//! sum the episode up once it ends. Readers place every message by its log time,
//! so summary messages written last still open the recording.
use super::capture::{EpisodeCompletion, EpisodeError, EpisodeStart, EpisodeSummary, Sample};
use super::mcap_schema::{McapMessage, RecordingSchema};
use base64::Engine as _;
use mcap::records::MessageHeader;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use spiral_robotics_common::datasets::{EpisodeVideo, H264AccessUnit, VideoPacket};
use std::collections::BTreeMap;
use std::io::{Seek, Write};

/// Hard storage limit for a single recorded episode.
const MAX_EPISODE_FRAMES: u64 = 1_000_000;
const COMPRESSED_VIDEO_SCHEMA: &[u8] = include_bytes!("schemas/compressed-video.bin");

/// One camera an episode films: its stream, the camera behind it, and the
/// width, height, and rate it is encoded at.
#[derive(Clone, Debug)]
pub struct CameraStream {
    stream: String,
    camera_id: String,
    dimensions: (u32, u32, u32),
}

impl CameraStream {
    pub fn new(
        stream: impl Into<String>,
        camera_id: impl Into<String>,
        dimensions: (u32, u32, u32),
    ) -> Self {
        Self {
            stream: stream.into(),
            camera_id: camera_id.into(),
            dimensions,
        }
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    fn topic(&self) -> String {
        format!("/camera/{}/video", self.stream)
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

/// One encoded frame of a camera stream, at its time since the episode began.
#[derive(Clone, Debug)]
pub struct VideoUnit {
    stream: String,
    elapsed_ns: u64,
    data: Vec<u8>,
}

impl VideoUnit {
    pub fn new(stream: impl Into<String>, elapsed_ns: u64, data: Vec<u8>) -> Self {
        Self {
            stream: stream.into(),
            elapsed_ns,
            data,
        }
    }
}

/// Splits an Annex B H.264 byte stream into access units as its bytes arrive.
/// Each access unit begins with a delimiter, which the encoder writes before
/// every frame.
#[derive(Default)]
pub struct AccessUnits {
    pending: Vec<u8>,
    /// Where scanning resumes; every start code before it is accounted for.
    scanned: usize,
}

impl AccessUnits {
    /// Take `bytes`, and answer every access unit they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        let mut units = Vec::new();
        let mut index = self.scanned;
        while index + 3 < self.pending.len() {
            let prefix = if self.pending[index..].starts_with(&[0, 0, 0, 1]) {
                4
            } else if self.pending[index..].starts_with(&[0, 0, 1]) {
                3
            } else {
                index += 1;
                continue;
            };
            if index + prefix >= self.pending.len() {
                // The unit type is in bytes still to come.
                break;
            }
            if index > 0 && self.pending[index + prefix] & 31 == 9 {
                units.push(self.pending.drain(..index).collect());
                index = 0;
            }
            index += prefix;
        }
        self.scanned = index;
        units
    }

    /// The last access unit, once the stream ends.
    pub fn finish(self) -> Option<Vec<u8>> {
        (!self.pending.is_empty()).then_some(self.pending)
    }
}

/// An episode's recording as it is written to `W`.
pub(crate) struct EpisodeRecording<W: Write + Seek> {
    writer: mcap::Writer<W>,
    start: EpisodeStart,
    origin_ns: u64,
    schema: RecordingSchema,
    video: VideoChannels,
    frames: u64,
}

/// The channel each camera's frames are written on, and the frames' schema.
struct VideoChannels {
    frame: MessageDescriptor,
    clock: MessageDescriptor,
    /// By stream: the channel, and the next frame's sequence number.
    channels: BTreeMap<String, (u16, u32)>,
}

impl<W: Write + Seek> EpisodeRecording<W> {
    /// Start `start`'s recording in `target`, with a channel for each of
    /// `cameras`.
    pub(crate) fn new(
        target: W,
        start: EpisodeStart,
        cameras: &[CameraStream],
    ) -> Result<Self, String> {
        let origin_ns = u64::try_from(start.started_ms())
            .map_err(|_| "Recording time predates Unix epoch")?
            .checked_mul(1_000_000)
            .ok_or("Recording origin overflows nanoseconds")?;
        let mut writer = mcap::WriteOptions::new()
            .compression(Some(mcap::Compression::Lz4))
            .create(target)
            .map_err(|e| e.to_string())?;
        let pool = DescriptorPool::decode(COMPRESSED_VIDEO_SCHEMA).map_err(|e| e.to_string())?;
        let message = |name: &str| {
            pool.get_message_by_name(name)
                .ok_or_else(|| format!("{name} schema is missing"))
        };
        let mut video = VideoChannels {
            frame: message("foxglove.CompressedVideo")?,
            clock: message("google.protobuf.Timestamp")?,
            channels: BTreeMap::new(),
        };
        let schema_id = writer
            .add_schema(
                "foxglove.CompressedVideo",
                "protobuf",
                COMPRESSED_VIDEO_SCHEMA,
            )
            .map_err(|e| e.to_string())?;
        for camera in cameras {
            let (width, height, _) = camera.dimensions;
            let metadata = BTreeMap::from([
                ("width".into(), width.to_string()),
                ("height".into(), height.to_string()),
            ]);
            let channel = writer
                .add_channel(schema_id, &camera.topic(), "protobuf", &metadata)
                .map_err(|e| e.to_string())?;
            video.channels.insert(camera.stream.clone(), (channel, 0));
        }
        Ok(Self {
            writer,
            start,
            origin_ns,
            schema: RecordingSchema::load()?,
            video,
            frames: 0,
        })
    }

    /// Write the episode's next joint sample.
    pub(crate) fn sample(&mut self, sample: &Sample) -> Result<(), String> {
        if self.frames >= MAX_EPISODE_FRAMES {
            return Err(format!(
                "recording reached the supported {MAX_EPISODE_FRAMES}-sample limit"
            ));
        }
        let messages = self
            .start
            .joint_sample_messages(sample, self.frames, &self.schema)
            .map_err(|error: EpisodeError| error.to_string())?;
        for message in &messages {
            self.write(message)?;
        }
        self.frames += 1;
        Ok(())
    }

    /// Write one camera's next encoded frame.
    pub(crate) fn video(&mut self, unit: &VideoUnit) -> Result<(), String> {
        let (channel, sequence) = self
            .video
            .channels
            .get_mut(&unit.stream)
            .ok_or_else(|| format!("the episode films no camera {}", unit.stream))?;
        let timestamp = self
            .origin_ns
            .checked_add(unit.elapsed_ns)
            .ok_or("Camera timestamp overflow")?;
        let mut time = DynamicMessage::new(self.video.clock.clone());
        time.set_field_by_name("seconds", Value::I64((timestamp / 1_000_000_000) as i64));
        time.set_field_by_name("nanos", Value::I32((timestamp % 1_000_000_000) as i32));
        let mut message = DynamicMessage::new(self.video.frame.clone());
        message.set_field_by_name("timestamp", Value::Message(time));
        message.set_field_by_name("frame_id", Value::String(unit.stream.clone()));
        message.set_field_by_name("format", Value::String("h264".into()));
        message.set_field_by_name("data", Value::Bytes(unit.data.clone().into()));
        let header = MessageHeader {
            channel_id: *channel,
            sequence: *sequence,
            log_time: timestamp,
            publish_time: timestamp,
        };
        *sequence = sequence.checked_add(1).ok_or("Camera sequence overflow")?;
        self.writer
            .write_to_known_channel(&header, &message.encode_to_vec())
            .map_err(|e| e.to_string())
    }

    /// End the chunk being written, so everything written so far is in the
    /// target for a reader.
    pub(crate) fn flush(&mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| e.to_string())
    }

    /// Write the messages that sum the episode up and close the recording.
    pub(crate) fn finish(
        mut self,
        completion: &EpisodeCompletion,
        cameras: &[CameraStream],
    ) -> Result<W, String> {
        let summary = EpisodeSummary::new(self.start.clone(), completion.clone(), self.frames);
        self.write(
            &summary
                .episode_message(&self.schema)
                .map_err(|e| e.to_string())?,
        )?;
        for message in self.start.joint_stream_messages(completion, &self.schema) {
            self.write(&message)?;
        }
        for camera in cameras {
            self.write(&camera.stream_message(self.start.id(), &self.schema))?;
        }
        self.writer.finish().map_err(|e| e.to_string())?;
        Ok(self.writer.into_inner())
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
}

/// Timestamped camera access units, for tests that build a recording file.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub struct CameraRecording {
    camera: CameraStream,
    frames: Vec<VideoUnit>,
}

#[cfg(any(test, feature = "test-support"))]
impl CameraRecording {
    /// `encoded`'s access units, one for each of its timestamps.
    pub fn from_annex_b(
        stream: String,
        dimensions: (u32, u32, u32),
        encoded: (Vec<u8>, Vec<u64>),
    ) -> Result<Self, String> {
        let (bytes, times) = encoded;
        let mut units = AccessUnits::default();
        let mut data = units.push(&bytes);
        data.extend(units.finish());
        if data.len() != times.len() || times.is_empty() {
            return Err(format!(
                "Camera {stream} encoded {} access units for {} timestamps",
                data.len(),
                times.len()
            ));
        }
        let frames = times
            .into_iter()
            .zip(data)
            .map(|(elapsed_ns, data)| VideoUnit::new(stream.clone(), elapsed_ns, data))
            .collect();
        Ok(Self {
            camera: CameraStream::new(stream.clone(), stream, dimensions),
            frames,
        })
    }

    pub fn with_camera_id(mut self, camera_id: String) -> Self {
        self.camera.camera_id = camera_id;
        self
    }
}

/// Recording files built whole, for tests.
#[cfg(any(test, feature = "test-support"))]
pub struct McapRecording;

#[cfg(any(test, feature = "test-support"))]
impl McapRecording {
    /// `episode`'s samples and `cameras`' frames as one recording file.
    pub fn encode(
        episode: &crate::FullEpisode,
        cameras: &[CameraRecording],
    ) -> Result<Vec<u8>, String> {
        let streams = cameras
            .iter()
            .map(|camera| camera.camera.clone())
            .collect::<Vec<_>>();
        let mut recording = EpisodeRecording::new(
            std::io::Cursor::new(Vec::new()),
            episode.start().clone(),
            &streams,
        )?;
        for sample in episode.samples() {
            recording.sample(sample)?;
        }
        for frame in cameras.iter().flat_map(|camera| &camera.frames) {
            recording.video(frame)?;
        }
        Ok(recording
            .finish(episode.completion(), &streams)?
            .into_inner())
    }
}

const MAX_PACKETS: usize = 18_000;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

/// A camera's frames as a review plays them.
#[derive(Default)]
pub(crate) struct CameraPackets {
    packets: Vec<VideoPacket>,
    codec: Option<String>,
    previous_time: Option<u64>,
    encoded_bytes: usize,
}
impl CameraPackets {
    pub(crate) fn push(&mut self, timestamp: u64, bytes: &[u8]) -> Result<(), String> {
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

    pub(crate) fn finish(self) -> Result<EpisodeVideo, String> {
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

    #[test]
    fn access_units_split_at_each_delimiter_however_the_bytes_arrive() {
        let key: &[u8] = &[
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1,
            0x65, 0x88,
        ];
        let delta: &[u8] = &[0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x41, 0x88];
        let stream = [key, delta, delta].concat();
        for size in [1, 2, 3, 5, stream.len()] {
            let mut units = AccessUnits::default();
            let mut split = stream
                .chunks(size)
                .flat_map(|bytes| units.push(bytes))
                .collect::<Vec<_>>();
            split.extend(units.finish());
            assert_eq!(split, [key, delta, delta], "{size}-byte pieces");
        }
    }
}
