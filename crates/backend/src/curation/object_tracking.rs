//! The Modal service that tracks objects, as Track objects asks it: one
//! episode's camera packets and a prompt naming the objects, tracked with a
//! checkpoint on a service revision, and answered as a row per sampled
//! picture (five a second) in display order — its `timestamp_ns` from the
//! episode's first message, and the `objects` tracked in it: each its track
//! `id`, its `score`, and its box, `x` and `y` its top-left corner and
//! `width` and `height`, all as fractions of the picture.
use super::video_service::{VideoContract, VideoQuestion, VideoService};
use arrow_schema::{DataType, Field, Fields};
use spiral_robotics_storage::ProviderKeys;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// The object tracking service as the provider keys reach it.
pub(super) struct ObjectTrackingService(VideoService);

impl ObjectTrackingService {
    /// The service the provider keys name, sharing `permits` with every
    /// other run's.
    pub(super) fn of(keys: &ProviderKeys, permits: Arc<Semaphore>) -> Result<Self, String> {
        VideoService::of(
            Self::contract(),
            keys.object_tracking_url(),
            keys.modal_token(),
            permits,
        )
        .map(Self)
    }

    /// Tracking the objects `prompt` names with the checkpoint `model` on the
    /// service `revision`.
    pub(super) fn tracking<'a>(
        &'a self,
        prompt: &'a str,
        model: &'a str,
        revision: &'a str,
    ) -> VideoQuestion<'a> {
        self.0.question(vec![
            ("model", model),
            ("implementation", revision),
            ("prompt", prompt),
        ])
    }

    fn contract() -> VideoContract {
        VideoContract::new(
            "Track objects",
            "OBJECT_TRACKING_URL",
            "track_objects",
            Self::picture_fields(),
        )
    }

    /// The fields of one sampled picture's tracks, as the contract types
    /// them; a picture may track no object. Nested within a row, an object
    /// and its fields are declared nullable, so the service may declare them
    /// either way.
    fn picture_fields() -> Fields {
        let object = Fields::from(vec![
            Field::new("id", DataType::UInt32, true),
            Field::new("score", DataType::Float32, true),
            Field::new("x", DataType::Float32, true),
            Field::new("y", DataType::Float32, true),
            Field::new("width", DataType::Float32, true),
            Field::new("height", DataType::Float32, true),
        ]);
        Fields::from(vec![
            Field::new("timestamp_ns", DataType::UInt64, false),
            Field::new(
                "objects",
                DataType::new_list(DataType::Struct(object), true),
                false,
            ),
        ])
    }
}

#[cfg(test)]
mod tests;
