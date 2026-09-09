//! Read control/review samples from the original MCAP Blob; no recording tables.
use super::{
    capture::{EpisodeError, FullEpisode},
    episode_schema::{EpisodeSchema, EpisodeStreamCatalog, EpisodeStreamRow, StoredEpisodeRow},
    mcap_schema::McapSchema,
};
use crate::{ProjectStore, db::SpiralSql};
use arrow_array::{Array as _, BinaryArray, BinaryViewArray, LargeBinaryArray, RecordBatch};
use arrow_schema::Schema;
use std::{io::Cursor, sync::Arc};

pub(super) struct McapRead;
impl McapRead {
    pub(super) async fn load(store: &ProjectStore, id: &str) -> Result<FullEpisode, EpisodeError> {
        let mut connection = store.connect().await.map_err(EpisodeError::Invalid)?;
        let sql = format!(
            "SELECT spql.io.read_bytes(source) AS data FROM dataset_source WHERE recording = {} LIMIT 2",
            SpiralSql::string_literal(&format!("{id}.mcap"))
        );
        let batches = connection
            .read_batches(&sql)
            .await
            .map_err(EpisodeError::Invalid)?;
        if batches.iter().map(RecordBatch::num_rows).sum::<usize>() != 1 {
            return Err(EpisodeError::Invalid(
                "Recording source is absent or ambiguous across datasets".into(),
            ));
        }
        let batch = batches
            .iter()
            .find(|batch| batch.num_rows() == 1)
            .ok_or_else(|| EpisodeError::NotFound(id.into()))?;
        let array = batch
            .column_by_name("data")
            .ok_or_else(|| EpisodeError::Invalid("MCAP source bytes are missing".into()))?;
        let bytes = if let Some(array) = array.as_any().downcast_ref::<BinaryArray>() {
            array.value(0)
        } else if let Some(array) = array.as_any().downcast_ref::<BinaryViewArray>() {
            array.value(0)
        } else if let Some(array) = array.as_any().downcast_ref::<LargeBinaryArray>() {
            array.value(0)
        } else {
            return Err(EpisodeError::Invalid("MCAP source is not binary".into()));
        };
        Self::decode(bytes).map_err(EpisodeError::Invalid)
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<FullEpisode, String> {
        let header_schema = McapSchema::recording("Episode")?;
        let stream_schema = McapSchema::recording("Stream")?;
        let joint_schema = McapSchema::recording("JointSample")?;
        let mut headers = Vec::new();
        let mut streams = Vec::new();
        let mut samples = Vec::new();
        for message in mcap::MessageStream::new(bytes).map_err(|e| e.to_string())? {
            let message = message.map_err(|e| e.to_string())?;
            if message.channel.topic == "/studio/episode" {
                headers.push(header_schema.decode(&message.data)?);
            } else if message.channel.topic.starts_with("/studio/streams/") {
                streams.push(stream_schema.decode(&message.data)?);
            } else if message.channel.topic.starts_with("/studio/joints/") {
                samples.push(joint_schema.decode(&message.data)?);
            }
        }
        if headers.len() != 1 {
            return Err("MCAP must contain one Studio recording header for robot replay".into());
        }
        let stream_batches = Self::batches(EpisodeSchema::episode_streams(), streams)?;
        let mut stream_rows = Vec::new();
        for batch in stream_batches {
            for row in 0..batch.num_rows() {
                stream_rows.push(EpisodeStreamRow::decode(&batch, row)?);
            }
        }
        let stream_catalog = EpisodeStreamCatalog::new(stream_rows);
        let headers = Self::batches(EpisodeSchema::episodes(), headers)?;
        let (start, completion, frames) =
            StoredEpisodeRow::decode(&headers[0], 0, &stream_catalog)?.into_parts();
        let samples = start.decode_samples(samples)?;
        if samples.len() as u64 != frames {
            return Err("MCAP joint sample count differs from its recording header".into());
        }
        Ok(FullEpisode::from_recording(start, samples, completion))
    }
    fn batches(
        schema: Arc<Schema>,
        rows: Vec<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Vec<RecordBatch>, String> {
        let mut encoded = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut encoded, &row).map_err(|e| e.to_string())?;
            encoded.push(b'\n');
        }
        arrow_json::ReaderBuilder::new(schema)
            .build(Cursor::new(encoded))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
}
