//! The Modal service that scores progress, as Score progress asks it: one
//! episode's camera packets, scored with a checkpoint on a service revision,
//! and answered as a score per picture in display order.
use super::video_service::{VideoContract, VideoQuestion, VideoService};
use arrow_schema::{DataType, Field, Fields};
use spiral_robotics_storage::ProviderKeys;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// The progress service as the provider keys reach it.
pub(super) struct ProgressService(VideoService);

impl ProgressService {
    /// The service the provider keys name, sharing `permits` with every
    /// other run's.
    pub(super) fn of(keys: &ProviderKeys, permits: Arc<Semaphore>) -> Result<Self, String> {
        VideoService::of(
            Self::contract(),
            keys.progress_service_url(),
            keys.modal_token(),
            permits,
        )
        .map(Self)
    }

    /// Scoring with the checkpoint `model` on the service `revision`.
    pub(super) fn scoring(&self, model: &str, revision: &str) -> VideoQuestion<'_> {
        self.0.question(vec![
            ("model", model.to_owned()),
            ("implementation", revision.to_owned()),
        ])
    }

    fn contract() -> VideoContract {
        VideoContract::new(
            "Score progress",
            "WARP_PROGRESS_URL",
            "score_progress",
            Self::score_fields(),
        )
    }

    /// The fields of one picture's score, as the contract types them.
    fn score_fields() -> Fields {
        Fields::from(vec![
            Field::new("frame_index", DataType::UInt32, false),
            Field::new("timestamp_ns", DataType::UInt64, false),
            Field::new("velocity", DataType::Float32, false),
            Field::new("progress", DataType::Float32, true),
            Field::new("window_stride", DataType::UInt32, false),
            Field::new("support", DataType::UInt32, false),
        ])
    }
}

#[cfg(test)]
pub(super) mod tests;
