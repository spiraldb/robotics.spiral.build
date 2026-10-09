//! An episode's review, read back from its file: each joint sample's positions,
//! or one camera's frames. The file is read a chunk at a time, so a review
//! holds only what it answers, never the whole episode.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read as _;
use std::path::PathBuf;

use mcap::sans_io::{LinearReadEvent, LinearReader, LinearReaderOptions};
use prost_reflect::DescriptorPool;
use spiral_robotics_common::datasets::EpisodeVideo;
use spiral_robotics_common::recording::{EpisodeInspectionArm, EpisodeInspectionSnapshot};

use super::capture::EpisodeSummary;
use super::mcap_recording::CameraPackets;
use super::mcap_schema::{RecordedMessage, RecordingSchema};

const COMPRESSED_VIDEO_SCHEMA: &[u8] = include_bytes!("schemas/compressed-video.bin");

/// The file of an episode, finished or paused, as its review reads it.
pub struct EpisodeFileReader {
    path: PathBuf,
}

impl EpisodeFileReader {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Every joint sample's time and each arm's normalized positions, for the
    /// episode `summary` describes.
    pub fn inspection(
        &self,
        summary: &EpisodeSummary,
    ) -> Result<EpisodeInspectionSnapshot, String> {
        let schema = RecordingSchema::load()?;
        let descriptor = schema.descriptor("JointSample");
        let arms = summary.start().arms();
        let topics = arms
            .iter()
            .enumerate()
            .map(|(index, arm)| (format!("/studio/joints/{}", arm.id()), index))
            .collect::<BTreeMap<_, _>>();
        let mut times = Vec::new();
        let mut positions = vec![Vec::new(); arms.len()];
        self.scan(|topic, _, data| {
            let Some(&arm) = topics.get(topic) else {
                return Ok(());
            };
            let sample = RecordedMessage::decode(descriptor.clone(), data)?;
            if arm == 0 {
                let nanos = sample
                    .u64("episode_time_ns")
                    .ok_or("a joint sample has no time")?;
                times.push(nanos as f64 / 1e9);
            }
            positions[arm].extend(sample.floats("state_position"));
            Ok(())
        })?;
        let arms = arms
            .iter()
            .zip(positions)
            .map(|(arm, normalized)| {
                EpisodeInspectionArm::new(arm.id(), arm.joints().to_vec(), normalized)
            })
            .collect();
        Ok(EpisodeInspectionSnapshot::new(summary.info(), times, arms))
    }

    /// Camera `stream`'s frames, on the clock of the episode `summary` describes.
    pub fn camera(&self, summary: &EpisodeSummary, stream: &str) -> Result<EpisodeVideo, String> {
        let origin_ns = u64::try_from(summary.start().started_ms())
            .map_err(|_| "Recording time predates Unix epoch")?
            .saturating_mul(1_000_000);
        let descriptor = DescriptorPool::decode(COMPRESSED_VIDEO_SCHEMA)
            .map_err(|error| error.to_string())?
            .get_message_by_name("foxglove.CompressedVideo")
            .ok_or("CompressedVideo schema is missing")?;
        let topic = format!("/camera/{stream}/video");
        let mut packets = CameraPackets::default();
        self.scan(|channel, log_time, data| {
            if channel != topic {
                return Ok(());
            }
            let frame = prost_reflect::DynamicMessage::decode(descriptor.clone(), data)
                .map_err(|error| error.to_string())?;
            let bytes = frame
                .get_field_by_name("data")
                .and_then(|data| data.as_bytes().cloned())
                .ok_or("a camera frame has no data")?;
            packets.push(log_time.saturating_sub(origin_ns), &bytes)
        })?;
        packets.finish()
    }

    /// Visit every message in the file with its topic, log time, and bytes.
    /// A paused episode's file ends after its last whole chunk.
    fn scan(
        &self,
        mut visit: impl FnMut(&str, u64, &[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut file = File::open(&self.path)
            .map_err(|error| format!("could not open {}: {error}", self.path.display()))?;
        let mut reader = LinearReader::new_with_options(
            LinearReaderOptions::default().with_skip_end_magic(true),
        );
        let mut topics = BTreeMap::new();
        while let Some(event) = reader.next_event() {
            match event.map_err(|error| error.to_string())? {
                LinearReadEvent::ReadRequest(wanted) => {
                    let read = file
                        .read(reader.insert(wanted))
                        .map_err(|error| error.to_string())?;
                    reader.notify_read(read);
                }
                LinearReadEvent::Record { data, opcode } => {
                    match mcap::parse_record(opcode, data).map_err(|error| error.to_string())? {
                        mcap::records::Record::Channel(channel) => {
                            topics.insert(channel.id, channel.topic);
                        }
                        mcap::records::Record::Message { header, data } => {
                            if let Some(topic) = topics.get(&header.channel_id) {
                                visit(topic, header.log_time, &data)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::recording::EpisodeArm;

    use super::*;
    use crate::{CameraStream, EpisodeCompletion, EpisodeFile, EpisodeStart, Sample, VideoUnit};

    #[test]
    fn a_review_reads_back_what_the_episode_wrote_paused_and_finished() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("episode-1.mcap");
        let start = EpisodeStart::new("episode-1".to_owned(), "data".to_owned())
            .with_timing(1_700_000_000_000, 60.0)
            .with_arms(
                vec![EpisodeArm::new("arm", None, vec!["a".into(), "b".into()])],
                vec![],
            );
        let file = EpisodeFile::create(
            path.clone(),
            &start,
            vec![CameraStream::new("wrist", "camera-1", (640, 480, 30))],
        )
        .unwrap();
        let (sink, video) = (file.sink(), file.video_sink());
        assert!(sink.sample(Sample::new(
            0.0,
            vec![vec![Some(0.1), None]],
            vec![vec![1, 2]]
        )));
        assert!(sink.sample(Sample::new(
            0.5,
            vec![vec![Some(0.3), Some(0.4)]],
            vec![vec![3, 4]]
        )));
        let key = vec![
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1,
            0x65, 0x88,
        ];
        let delta = vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x41, 0x88];
        assert!(video.send(VideoUnit::new("wrist", 0, key)));
        assert!(video.send(VideoUnit::new("wrist", 33_333_333, delta)));
        let summary = EpisodeSummary::new(start, EpisodeCompletion::new(1.0, vec![], vec![]), 2);
        let reader = EpisodeFileReader::new(path);
        let read = |reader: &EpisodeFileReader| {
            let inspection = reader.inspection(&summary).unwrap();
            let camera = reader.camera(&summary, "wrist").unwrap();
            (
                inspection.sample_times_seconds().to_vec(),
                inspection.arms()[0].normalized().to_vec(),
                camera
                    .packets()
                    .iter()
                    .map(|packet| (packet.timestamp_ns(), packet.keyframe()))
                    .collect::<Vec<_>>(),
            )
        };
        let expected = (
            vec![0.0, 0.5],
            vec![Some(0.1), None, Some(0.3), Some(0.4)],
            vec![(0, true), (33_333_333, false)],
        );

        // Paused: everything flushed reads back, with no footer yet.
        sink.flush().unwrap().blocking_recv().unwrap().unwrap();
        assert_eq!(read(&reader), expected);
        file.finish(summary.clone()).unwrap();
        assert_eq!(read(&reader), expected);
    }
}
