//! One self-contained MCAP recording: joint messages, metadata, and H.264 access units.
use super::episode_schema::{
    JointSampleRows as _, SpiralEpisodeEncoder, StoredEpisodeRow, VideoStreamProperties,
    VideoStreamRow, VideoStreamRows as _,
};
use super::episodes::FullEpisode;
use super::mcap_schema::McapSchema;
use crate::db::ArrowColumns;
use arrow_array::RecordBatch;
use mcap::records::MessageHeader;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, Value};
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

#[cfg(test)]
mod tests {
    use super::super::mcap_read::McapRead;
    use super::*;
    use crate::episodes::{EpisodeCompletion, EpisodeFixture, Sample};
    use spiral_robotics_common::recording::EpisodeArm;

    #[test]
    fn one_mcap_preserves_joint_nulls_actions_and_camera_timestamps() {
        let fixture = EpisodeFixture::new("take-1", "default")
            .with_timing(1_700_000_000_000, 30.0)
            .with_arms(vec![EpisodeArm::new(
                "arm",
                None,
                vec!["shoulder".into(), "elbow".into()],
            )])
            .with_duration_seconds(0.3)
            .into_episode();
        let samples = vec![
            Sample::new(0.025, vec![vec![Some(0.25), None]], vec![vec![100, 200]])
                .with_action(vec![Some(vec![Some(0.5), None])]),
        ];
        let episode = FullEpisode::from_recording(
            fixture.start().clone(),
            samples,
            EpisodeCompletion::new(0.3, vec![], vec![]),
        );
        let times = (0..8)
            .map(|index| 10_000_000 + index * 33_333_333)
            .collect::<Vec<_>>();
        let camera = CameraRecording::from_annex_b(
            "wrist".into(),
            (32, 32, 30),
            (
                include_bytes!("../../../../frontend/src/activities/data/fixtures/seek.h264")
                    .to_vec(),
                times.clone(),
            ),
        )
        .unwrap();
        let bytes = McapRecording::encode(&episode, &[camera]).unwrap();
        let decoded = McapRead::decode(&bytes).unwrap();
        assert_eq!(decoded.summary(), episode.summary());
        assert_eq!(
            decoded.samples()[0].normalized(),
            episode.samples()[0].normalized()
        );
        assert_eq!(decoded.samples()[0].action(), episode.samples()[0].action());
        let video = mcap::MessageStream::new(&bytes)
            .unwrap()
            .map(Result::unwrap)
            .filter(|message| message.channel.topic == "/camera/wrist/video")
            .collect::<Vec<_>>();
        assert_eq!(
            video
                .iter()
                .map(|message| message.log_time - 1_700_000_000_000_000_000)
                .collect::<Vec<_>>(),
            times
        );
        assert!(
            video
                .iter()
                .all(|message| message.log_time == message.publish_time)
        );
    }
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
        let mut packets = crate::datasets::CameraPackets::default();
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
}

pub(crate) struct McapRecording {
    writer: mcap::Writer<Cursor<Vec<u8>>>,
    origin_ns: u64,
}
impl McapRecording {
    pub(crate) fn encode(
        episode: &FullEpisode,
        cameras: &[CameraRecording],
    ) -> Result<Vec<u8>, String> {
        let origin_ns = u64::try_from(episode.start().started_ms())
            .map_err(|_| "Recording time predates Unix epoch")?
            .checked_mul(1_000_000)
            .ok_or("Recording origin overflows nanoseconds")?;
        let mut recording = Self {
            writer: mcap::Writer::new(Cursor::new(Vec::new())).map_err(|e| e.to_string())?,
            origin_ns,
        };
        let row = StoredEpisodeRow::new(
            episode.start().clone(),
            episode.completion().clone(),
            episode.samples().len() as u64,
        );
        recording.rows(
            "Episode",
            &StoredEpisodeRow::batch(&[row]).map_err(|e| e.to_string())?,
        )?;
        recording.rows(
            "Stream",
            &episode
                .start()
                .streams_batch(episode.completion())
                .map_err(|e| e.to_string())?,
        )?;
        let mut encoder =
            SpiralEpisodeEncoder::new(episode.start().clone()).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        for sample in episode.samples() {
            rows.extend(encoder.encode(sample).map_err(|e| e.to_string())?);
            if rows.len() >= 512 {
                recording.rows("JointSample", &rows.batch().map_err(|e| e.to_string())?)?;
                rows.clear();
            }
        }
        if !rows.is_empty() {
            recording.rows("JointSample", &rows.batch().map_err(|e| e.to_string())?)?;
        }
        let mut streams = std::collections::BTreeSet::new();
        for camera in cameras {
            if streams.insert(camera.stream.clone()) {
                let row = VideoStreamRow::new(
                    episode.start().id().into(),
                    camera.stream.clone(),
                    VideoStreamProperties::new(
                        camera.camera_id.clone(),
                        super::video::VideoSize::new(camera.dimensions.0, camera.dimensions.1),
                        camera.dimensions.2,
                    ),
                );
                recording.rows("Stream", &vec![row].batch().map_err(|e| e.to_string())?)?;
            }
            recording.camera(camera)?;
        }
        recording.writer.finish().map_err(|e| e.to_string())?;
        Ok(recording.writer.into_inner().into_inner())
    }
    fn rows(&mut self, name: &str, batch: &RecordBatch) -> Result<(), String> {
        let schema = McapSchema::recording(name)?;
        let schema_id = self
            .writer
            .add_schema(schema.name(), "protobuf", schema.bytes())
            .map_err(|e| e.to_string())?;
        let columns = ArrowColumns::new(batch, "MCAP recording");
        for row in 0..batch.num_rows() {
            let (topic, elapsed_ns, sequence) = match name {
                "Episode" => ("/studio/episode".to_owned(), 0, 0),
                "Stream" => (
                    format!("/studio/streams/{}", columns.text("stream_id", row)?),
                    0,
                    columns.unsigned("ordinal", row)?,
                ),
                "JointSample" => (
                    format!("/studio/joints/{}", columns.text("stream_id", row)?),
                    columns.unsigned("episode_time_ns", row)?,
                    columns.unsigned("sample_index", row)?,
                ),
                _ => return Err("Unknown recording message".into()),
            };
            let channel_id = self
                .writer
                .add_channel(schema_id, &topic, "protobuf", &BTreeMap::new())
                .map_err(|e| e.to_string())?;
            let timestamp = self
                .origin_ns
                .checked_add(elapsed_ns)
                .ok_or("Recording timestamp overflow")?;
            let header = MessageHeader {
                channel_id,
                sequence: u32::try_from(sequence).map_err(|_| "Recording sequence overflow")?,
                log_time: timestamp,
                publish_time: timestamp,
            };
            self.writer
                .write_to_known_channel(&header, &schema.encode(batch, row)?)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
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
