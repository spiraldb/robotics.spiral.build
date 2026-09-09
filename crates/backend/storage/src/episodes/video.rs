//! Persisted video values shared by camera capture and episode storage.

use std::sync::Arc;

use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, LargeBinaryArray, RecordBatch, StringArray,
    StringViewArray, StructArray, UInt32Array, UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::db::DatabaseSession as _;
use crate::db::SpiralSql;
use crate::db::column as db;
use crate::db::{ProjectConnection, ProjectStore};

const BLOB_EXTENSION_NAME: &str = "spql.io.Blob";

/// Browser-facing video asset reads for one project.
pub(crate) struct VideoAssetPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> VideoAssetPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn size(&self, asset: &VideoAssetKey) -> Result<u64, String> {
        let sql = SpiralSql::video_asset_size(self.project_store.table_prefix(), asset);
        self.u64_query(sql, db::BYTE_SIZE).await
    }

    pub(crate) async fn range(
        &self,
        asset: &VideoAssetKey,
        range: VideoByteRange,
    ) -> Result<Vec<u8>, String> {
        let sql = SpiralSql::video_asset_range(self.project_store.table_prefix(), asset, range);
        self.binary_query(sql).await
    }

    async fn record_batches(&self, sql: String) -> Result<crate::RecordBatchStream, String> {
        let mut connection = self.project_store.connect().await?;
        connection
            .query(sql.into())
            .await
            .map_err(|error| error.to_string())
    }

    async fn binary_query(&self, sql: String) -> Result<Vec<u8>, String> {
        let mut records = self.record_batches(sql).await?;
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral video: {error}"))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let payload = batch
                .column_by_name(db::PAYLOAD)
                .ok_or_else(|| format!("Spiral video query returned no {} column", db::PAYLOAD))?;
            if payload.is_null(0) {
                return Err("Spiral video payload is null".to_owned());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<BinaryArray>() {
                return Ok(payload.value(0).to_vec());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<LargeBinaryArray>() {
                return Ok(payload.value(0).to_vec());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<BinaryViewArray>() {
                return Ok(payload.value(0).to_vec());
            }
            return Err(format!(
                "Spiral video payload has unexpected type {}",
                payload.data_type()
            ));
        }
        Err("No video matched that episode and stream".to_owned())
    }

    async fn u64_query(&self, sql: String, column: &str) -> Result<u64, String> {
        let mut records = self.record_batches(sql).await?;
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral scalar result: {error}"))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let values = batch
                .column_by_name(column)
                .and_then(|array| array.as_any().downcast_ref::<UInt64Array>())
                .ok_or_else(|| format!("Spiral scalar query returned no UInt64 {column} column"))?;
            if values.is_null(0) {
                return Err(format!("Spiral scalar query returned null {column}"));
            }
            return Ok(values.value(0));
        }
        Err("Spiral scalar query returned no rows".to_owned())
    }
}

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

    pub(crate) fn duration_seconds(self) -> f64 {
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

    #[cfg(any(test, feature = "test-support"))]
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

    pub(crate) fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub(crate) fn camera_id(&self) -> &str {
        &self.camera_id
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) const fn size(&self) -> VideoSize {
        self.size
    }

    pub(crate) const fn timing(&self) -> VideoTiming {
        self.timing
    }

    pub(crate) const fn segment(&self) -> VideoSegment {
        self.segment
    }
    #[cfg(any(test, feature = "test-support"))]
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
    pub(crate) fn new(stream_id: String, camera_id: String, duration_seconds: f64) -> Self {
        Self {
            stream_id,
            camera_id,
            size: VideoSize::new(0, 0),
            timing: VideoTiming::new(0, 0),
            duration_seconds,
            segment: VideoSegment::new(0, None),
        }
    }

    pub(crate) fn with_size(mut self, size: VideoSize) -> Self {
        self.size = size;
        self
    }

    pub(crate) fn with_timing(mut self, timing: VideoTiming) -> Self {
        self.timing = timing;
        self
    }

    pub(crate) fn with_segment(mut self, segment: VideoSegment) -> Self {
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

    pub(crate) fn episode_id(&self) -> &str {
        &self.episode_id
    }

    pub(crate) fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub(crate) const fn segment_index(&self) -> Option<u32> {
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

    pub(crate) const fn offset(self) -> u64 {
        self.offset
    }

    pub(crate) const fn size(self) -> u64 {
        self.size
    }
}

impl VideoAsset {
    pub(crate) fn storage_publication_batch(
        episode_id: &str,
        assets: &[Self],
        media_relation: &str,
    ) -> Result<RecordBatch, String> {
        let videos = assets;
        let payloads = videos.iter().map(VideoAsset::payload).collect::<Vec<_>>();
        let timeline_offsets = videos
            .iter()
            .map(|video| {
                video
                    .segment()
                    .timeline_offset_seconds()
                    .map(|seconds| {
                        if !seconds.is_finite() || seconds < 0.0 {
                            return Err(format!(
                                "video timeline offset for {} must be finite and non-negative",
                                video.stream_id()
                            ));
                        }
                        let nanos = seconds * 1_000_000_000.0;
                        if nanos > u64::MAX as f64 {
                            return Err(format!(
                                "video timeline offset for {} is too large",
                                video.stream_id()
                            ));
                        }
                        Ok(nanos.round() as u64)
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let schema = VideoAssetStorage::schema();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(|_| episode_id),
            )),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(VideoAsset::stream_id),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.segment().index()),
            )),
            Arc::new(UInt64Array::from(timeline_offsets)),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(VideoAsset::camera_id),
            )),
            VideoAssetStorage::inline_blob_array(&payloads)?,
            Arc::new(UInt64Array::from_iter_values(
                payloads.iter().map(|payload| payload.len() as u64),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.size().width()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.size().height()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.timing().fps()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                videos.iter().map(|video| video.timing().frames()),
            )),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(|_| media_relation),
            )),
        ];
        RecordBatch::try_new(schema, columns)
            .map_err(|error| format!("building Spiral video batch: {error}"))
    }
}

pub(super) struct VideoAssetStorage;

impl VideoAssetStorage {
    pub(super) fn schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::STREAM_ID, DataType::Utf8, false),
            Field::new(db::SEGMENT_INDEX, DataType::UInt32, false),
            Field::new(db::TIMELINE_OFFSET_NS, DataType::UInt64, true),
            Field::new(db::CAMERA_ID, DataType::Utf8, false),
            Self::blob_field(db::PAYLOAD),
            Field::new(db::BYTE_SIZE, DataType::UInt64, false),
            Field::new(db::WIDTH, DataType::UInt32, false),
            Field::new(db::HEIGHT, DataType::UInt32, false),
            Field::new(db::FPS, DataType::UInt32, false),
            Field::new(db::FRAMES, DataType::UInt64, false),
            Field::new(db::MEDIA_RELATION, DataType::Utf8, false),
        ]))
    }

    fn blob_storage_fields() -> Fields {
        vec![
            Field::new("bytes", DataType::BinaryView, true),
            Field::new("uri", DataType::Utf8View, true),
            Field::new("offset", DataType::UInt64, true),
            Field::new("size", DataType::UInt64, true),
            Field::new("mime_type", DataType::Utf8View, true),
            Field::new("etag", DataType::Utf8View, true),
            Field::new("version_id", DataType::Utf8View, true),
            Field::new("generation", DataType::Utf8View, true),
            Field::new("md5", DataType::Utf8View, true),
            Field::new("sha1", DataType::Utf8View, true),
            Field::new("sha256", DataType::Utf8View, true),
            Field::new("manifest_digest", DataType::Utf8View, true),
        ]
        .into()
    }

    fn blob_field(name: &str) -> Field {
        Field::new(name, DataType::Struct(Self::blob_storage_fields()), true).with_metadata(
            [(
                EXTENSION_TYPE_NAME_KEY.to_owned(),
                BLOB_EXTENSION_NAME.to_owned(),
            )]
            .into(),
        )
    }

    fn inline_blob_array(payloads: &[&[u8]]) -> Result<ArrayRef, String> {
        let len = payloads.len();
        let fields = Self::blob_storage_fields();
        let null_text = || Arc::new(StringViewArray::from(vec![None::<&str>; len])) as ArrayRef;
        let null_u64 = || Arc::new(UInt64Array::from(vec![None::<u64>; len])) as ArrayRef;
        let sha256 = Arc::new(StringViewArray::from_iter_values(payloads.iter().map(
            |payload| {
                let digest = Sha256::digest(payload);
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            },
        ))) as ArrayRef;
        let values = vec![
            Arc::new(BinaryViewArray::from_iter_values(payloads.iter().copied())) as ArrayRef,
            null_text(),
            null_u64(),
            Arc::new(UInt64Array::from_iter_values(
                payloads.iter().map(|payload| payload.len() as u64),
            )) as ArrayRef,
            Arc::new(StringViewArray::from(vec![Some("video/mp4"); len])) as ArrayRef,
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            sha256,
            null_text(),
        ];
        StructArray::try_new(fields, values, None)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("building Spiral video Blob array: {error}"))
    }
}

pub(super) trait VideoAssetRecordBatch {
    fn normalize_video_asset(self) -> Result<RecordBatch, String>;
}

impl VideoAssetRecordBatch for RecordBatch {
    fn normalize_video_asset(self) -> Result<RecordBatch, String> {
        let batch = self;
        let payload_index = batch
            .schema()
            .index_of(db::PAYLOAD)
            .map_err(|error| format!("video asset batch is missing {}: {error}", db::PAYLOAD))?;
        let batch_schema = batch.schema();
        let payload_field = batch_schema.field(payload_index);
        if payload_field
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .is_some_and(|name| name == BLOB_EXTENSION_NAME)
        {
            return Ok(batch);
        }
        let payload = batch.column(payload_index);
        let values = if let Some(payload) = payload.as_any().downcast_ref::<BinaryArray>() {
            payload.iter().collect::<Vec<_>>()
        } else if let Some(payload) = payload.as_any().downcast_ref::<LargeBinaryArray>() {
            payload.iter().collect::<Vec<_>>()
        } else if let Some(payload) = payload.as_any().downcast_ref::<BinaryViewArray>() {
            payload.iter().collect::<Vec<_>>()
        } else {
            return Err(format!(
                "video asset {} must be a Spiral Blob or Arrow binary, got {}",
                db::PAYLOAD,
                payload.data_type(),
            ));
        };
        if values.iter().any(Option::is_none) {
            return Err("video asset payload cannot be null".to_owned());
        }
        let payloads = values.into_iter().flatten().collect::<Vec<_>>();
        let mut fields = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[payload_index] = VideoAssetStorage::blob_field(db::PAYLOAD);
        let schema = Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        ));
        let mut columns = batch.columns().to_vec();
        columns[payload_index] = VideoAssetStorage::inline_blob_array(&payloads)?;
        RecordBatch::try_new(schema, columns)
            .map_err(|error| format!("normalizing video asset Blob batch: {error}"))
    }
}

impl ProjectConnection {
    #[cfg(test)]
    pub(crate) async fn run_binary_query(&mut self, sql: &str) -> Result<Vec<u8>, String> {
        let mut records = self.execute_query(sql).await?;
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral video: {error}"))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let payload = batch
                .column_by_name(db::PAYLOAD)
                .ok_or_else(|| format!("Spiral video query returned no {} column", db::PAYLOAD))?;
            if payload.is_null(0) {
                return Err("Spiral video payload is null".to_owned());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<BinaryArray>() {
                return Ok(payload.value(0).to_vec());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<LargeBinaryArray>() {
                return Ok(payload.value(0).to_vec());
            }
            if let Some(payload) = payload.as_any().downcast_ref::<BinaryViewArray>() {
                return Ok(payload.value(0).to_vec());
            }
            return Err(format!(
                "Spiral video payload has unexpected type {}",
                payload.data_type()
            ));
        }
        Err("No video matched that episode and stream".to_owned())
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn inline_video_blobs_store_content_sha256_in_the_named_field() {
        let payload = b"spiral";
        let array = VideoAssetStorage::inline_blob_array(&[payload.as_slice()]).unwrap();
        let blob = array.as_any().downcast_ref::<StructArray>().unwrap();
        let sha256 = blob
            .column_by_name("sha256")
            .unwrap()
            .as_any()
            .downcast_ref::<StringViewArray>()
            .unwrap();
        let expected = Sha256::digest(payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(sha256.value(0), expected);

        let generation = blob
            .column_by_name("generation")
            .unwrap()
            .as_any()
            .downcast_ref::<StringViewArray>()
            .unwrap();
        assert!(generation.is_null(0));
    }
}
