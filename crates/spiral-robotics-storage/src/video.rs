//! Persisted video values shared by camera capture and episode storage.

use arrow_array::RecordBatch;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSize {
    width: u32,
    height: u32,
}

impl VideoSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoTiming {
    fps: u32,
    frames: u64,
}

impl VideoTiming {
    pub const fn new(fps: u32, frames: u64) -> Self {
        Self { fps, frames }
    }

    pub const fn fps(self) -> u32 {
        self.fps
    }

    pub const fn frames(self) -> u64 {
        self.frames
    }

    pub fn duration_seconds(self) -> f64 {
        if self.fps == 0 {
            0.0
        } else {
            self.frames as f64 / self.fps as f64
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoSegment {
    segment_index: u32,
    timeline_offset_seconds: Option<f64>,
}

impl VideoSegment {
    pub const fn new(segment_index: u32, timeline_offset_seconds: Option<f64>) -> Self {
        Self {
            segment_index,
            timeline_offset_seconds,
        }
    }

    pub const fn index(self) -> u32 {
        self.segment_index
    }

    pub const fn timeline_offset_seconds(self) -> Option<f64> {
        self.timeline_offset_seconds
    }
}

#[derive(Clone, Debug)]
pub struct VideoAsset {
    stream_id: String,
    camera_id: String,
    payload: Vec<u8>,
    size: VideoSize,
    timing: VideoTiming,
    segment: VideoSegment,
}

impl VideoAsset {
    pub fn new(stream_id: String, camera_id: String, payload: Vec<u8>) -> Self {
        Self {
            stream_id,
            camera_id,
            payload,
            size: VideoSize::new(0, 0),
            timing: VideoTiming::new(0, 0),
            segment: VideoSegment::new(0, None),
        }
    }

    pub fn publication_batch(
        episode_id: &str,
        assets: &[Self],
        media_relation: &str,
    ) -> Result<RecordBatch, String> {
        Self::storage_publication_batch(episode_id, assets, media_relation)
    }

    pub fn with_size(mut self, size: VideoSize) -> Self {
        self.size = size;
        self
    }

    pub fn with_timing(mut self, timing: VideoTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn with_segment(mut self, segment: VideoSegment) -> Self {
        self.segment = segment;
        self
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn camera_id(&self) -> &str {
        &self.camera_id
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub const fn size(&self) -> VideoSize {
        self.size
    }

    pub const fn timing(&self) -> VideoTiming {
        self.timing
    }

    pub const fn segment(&self) -> VideoSegment {
        self.segment
    }

    pub fn metadata(&self) -> VideoAssetMetadata {
        VideoAssetMetadata {
            stream_id: self.stream_id.clone(),
            camera_id: self.camera_id.clone(),
            size: self.size,
            timing: self.timing,
            duration_seconds: self.timing.duration_seconds(),
            segment: self.segment,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoAssetMetadata {
    stream_id: String,
    camera_id: String,
    #[serde(flatten)]
    size: VideoSize,
    #[serde(flatten)]
    timing: VideoTiming,
    duration_seconds: f64,
    #[serde(flatten)]
    segment: VideoSegment,
}

impl VideoAssetMetadata {
    pub fn new(stream_id: String, camera_id: String, duration_seconds: f64) -> Self {
        Self {
            stream_id,
            camera_id,
            size: VideoSize::new(0, 0),
            timing: VideoTiming::new(0, 0),
            duration_seconds,
            segment: VideoSegment::new(0, None),
        }
    }

    pub fn with_size(mut self, size: VideoSize) -> Self {
        self.size = size;
        self
    }

    pub fn with_timing(mut self, timing: VideoTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn with_segment(mut self, segment: VideoSegment) -> Self {
        self.segment = segment;
        self
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn camera_id(&self) -> &str {
        &self.camera_id
    }

    pub const fn size(&self) -> VideoSize {
        self.size
    }

    pub const fn timing(&self) -> VideoTiming {
        self.timing
    }

    pub const fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    pub const fn segment(&self) -> VideoSegment {
        self.segment
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoAssetKey {
    episode_id: String,
    stream_id: String,
    segment_index: Option<u32>,
}

impl VideoAssetKey {
    pub fn new(episode_id: String, stream_id: String, segment_index: Option<u32>) -> Self {
        Self {
            episode_id,
            stream_id,
            segment_index,
        }
    }

    pub fn episode_id(&self) -> &str {
        &self.episode_id
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub const fn segment_index(&self) -> Option<u32> {
        self.segment_index
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoByteRange {
    offset: u64,
    size: u64,
}

impl VideoByteRange {
    pub const fn new(offset: u64, size: u64) -> Self {
        Self { offset, size }
    }

    pub const fn offset(self) -> u64 {
        self.offset
    }

    pub const fn size(self) -> u64 {
        self.size
    }
}
