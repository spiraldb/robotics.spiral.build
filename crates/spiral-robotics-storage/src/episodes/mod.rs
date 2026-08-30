//! Episode-owned capture, relational encoding, queries, and video values.

mod episode_schema;
mod episodes;
pub(crate) mod video;

pub use episodes::*;

#[cfg(test)]
pub(crate) struct EpisodeFixture {
    start: EpisodeStart,
    duration_seconds: f64,
}

#[cfg(test)]
impl EpisodeFixture {
    pub(crate) fn new(episode_id: &str, dataset_id: &str) -> Self {
        Self {
            start: EpisodeStart::new(episode_id.to_owned(), dataset_id.to_owned())
                .with_source("transfer-e2e".to_owned(), None),
            duration_seconds: 0.0,
        }
    }

    pub(crate) fn with_timing(mut self, started_ms: i64, sample_hz: f64) -> Self {
        self.start = self.start.with_timing(started_ms, sample_hz);
        self
    }

    pub(crate) fn with_arms(
        mut self,
        arms: Vec<spiral_robotics_api::recording::EpisodeArm>,
    ) -> Self {
        self.start = self.start.with_arms(arms, Vec::new());
        self
    }

    pub(crate) const fn with_duration_seconds(mut self, duration_seconds: f64) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }

    pub(crate) fn into_episode(self) -> FullEpisode {
        FullEpisode::fixture_without_samples(
            self.start,
            EpisodeCompletion::new(self.duration_seconds, Vec::new(), Vec::new()),
        )
    }
}

use arrow_schema::{DataType, Field, Schema};

use crate::db::{ManagedRelationSchema, Table, column};
use episode_schema::EpisodeSchema;

/// Episode relations contributed to project and episode bundles.
pub(crate) struct EpisodeBundleSection;

impl EpisodeBundleSection {
    pub(crate) const RELATIONS: [Table; 5] = [
        Table::Datasets,
        Table::Episodes,
        Table::EpisodeStreams,
        Table::JointSamples,
        Table::VideoAssets,
    ];
    pub(super) const DELETION_RELATIONS: [Table; 4] = [
        Table::Episodes,
        Table::JointSamples,
        Table::EpisodeStreams,
        Table::VideoAssets,
    ];
}

impl ManagedRelationSchema for EpisodeBundleSection {
    fn relations(&self) -> &'static [Table] {
        &Self::RELATIONS
    }

    fn managed_relation_schema(&self, table: Table) -> Option<Schema> {
        let text = |name, nullable| Field::new(name, DataType::Utf8, nullable);
        match table {
            Table::Datasets => Some(EpisodeSchema::datasets().as_ref().clone()),
            Table::Episodes => Some(EpisodeSchema::episodes().as_ref().clone()),
            Table::EpisodeStreams => Some(EpisodeSchema::episode_streams().as_ref().clone()),
            Table::JointSamples => Some(EpisodeSchema::joint_samples().as_ref().clone()),
            // Portable files carry MP4 bytes as Arrow Binary. SpiralDB adapts
            // this field to its native Blob extension at the database edge.
            Table::VideoAssets => Some(Schema::new(vec![
                text(column::EPISODE_ID, false),
                text(column::STREAM_ID, false),
                Field::new(column::SEGMENT_INDEX, DataType::UInt32, false),
                Field::new(column::TIMELINE_OFFSET_NS, DataType::UInt64, true),
                text(column::CAMERA_ID, false),
                Field::new(column::PAYLOAD, DataType::Binary, true),
                Field::new(column::BYTE_SIZE, DataType::UInt64, false),
                Field::new(column::WIDTH, DataType::UInt32, false),
                Field::new(column::HEIGHT, DataType::UInt32, false),
                Field::new(column::FPS, DataType::UInt32, false),
                Field::new(column::FRAMES, DataType::UInt64, false),
                text(column::MEDIA_RELATION, false),
            ])),
            _ => None,
        }
    }
}

impl EpisodeBundleSection {
    pub(crate) fn dataset_schema() -> arrow_schema::SchemaRef {
        EpisodeSchema::datasets()
    }

    pub(crate) fn normalize_video_asset(
        batch: arrow_array::RecordBatch,
    ) -> Result<arrow_array::RecordBatch, String> {
        use video::VideoAssetRecordBatch as _;

        batch.normalize_video_asset()
    }

    #[cfg(test)]
    pub(crate) fn native_video_schema() -> std::sync::Arc<Schema> {
        video::VideoAssetStorage::schema()
    }
}
