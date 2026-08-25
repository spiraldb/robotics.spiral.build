//! Persisted video values shared by camera capture and episode storage.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct VideoAsset {
    pub stream_id: String,
    pub camera_id: String,
    pub payload: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VideoAssetMetadata {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

impl VideoAsset {
    pub fn metadata(&self) -> VideoAssetMetadata {
        VideoAssetMetadata {
            stream_id: self.stream_id.clone(),
            camera_id: self.camera_id.clone(),
            width: self.width,
            height: self.height,
            fps: self.fps,
            frames: self.frames,
            duration_seconds: if self.fps == 0 {
                0.0
            } else {
                self.frames as f64 / self.fps as f64
            },
            segment_index: self.segment_index,
            timeline_offset_seconds: self.timeline_offset_seconds,
        }
    }
}
