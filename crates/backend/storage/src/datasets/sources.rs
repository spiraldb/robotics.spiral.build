//! Publish one complete MCAP file through the Dataset's declared source view.
use crate::ProjectConnection;
use crate::StorageManager;
use arrow_array::{
    ArrayRef, BinaryViewArray, RecordBatch, StringArray, StringViewArray, StructArray, UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use spiral_robotics_common::curation::AnswerTable;
use spiral_robotics_common::sql::SqlLiteral;
use std::sync::Arc;

pub(crate) struct McapFile<'a> {
    dataset_name: &'a str,
    key: &'a str,
    bytes: &'a [u8],
}
impl<'a> McapFile<'a> {
    pub(crate) fn new(dataset_name: &'a str, key: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            dataset_name,
            key,
            bytes,
        }
    }
    fn batch(&self) -> Result<RecordBatch, String> {
        let text = || Arc::new(StringViewArray::from(vec![None::<&str>])) as ArrayRef;
        let schema = Arc::new(Schema::new(vec![
            arrow_schema::Field::new("dataset_name", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("recording", arrow_schema::DataType::Utf8, false),
            McapSourceBlob::field("source"),
        ]));
        let hash = format!("{:x}", Sha256::digest(self.bytes));
        let blob = StructArray::try_new(
            McapSourceBlob::fields(),
            vec![
                Arc::new(BinaryViewArray::from(vec![Some(self.bytes)])),
                text(),
                Arc::new(UInt64Array::from(vec![None::<u64>])),
                Arc::new(UInt64Array::from(vec![self.bytes.len() as u64])),
                Arc::new(StringViewArray::from(vec!["application/mcap"])),
                text(),
                text(),
                text(),
                text(),
                text(),
                Arc::new(StringViewArray::from(vec![hash.as_str()])),
                text(),
            ],
            None,
        )
        .map_err(|e| e.to_string())?;
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![self.dataset_name])),
                Arc::new(StringArray::from(vec![self.key])),
                Arc::new(blob),
            ],
        )
        .map_err(|e| e.to_string())
    }
    async fn publish(
        &self,
        connection: &mut crate::ProjectConnection,
        exists: bool,
    ) -> Result<(), String> {
        let relations = McapDatasetDefinition::new(self.dataset_name);
        let mut already_imported = false;
        if connection
            .table_exists(McapDatasetDefinition::SOURCE_TABLE)
            .await?
        {
            if !exists {
                let orphaned = connection
                    .query_rows(
                        &format!(
                            "SELECT recording FROM dataset_source WHERE dataset_name = {} LIMIT 1",
                            SqlLiteral::text(self.dataset_name)
                        ),
                        1,
                    )
                    .await?;
                if !orphaned.rows().is_empty() {
                    return Err("MCAP sources already exist without their Dataset; repair the catalog before appending files".into());
                }
            }
            let duplicate = connection.read_batches(&format!(
                "SELECT spql.io.read_bytes(source) AS payload FROM dataset_source WHERE dataset_name = {} AND recording = {} LIMIT 2",
                SqlLiteral::text(self.dataset_name), SqlLiteral::text(self.key)
            )).await?;
            let count = duplicate.iter().map(RecordBatch::num_rows).sum::<usize>();
            if count > 1 {
                return Err(
                    "Duplicate MCAP sources exist; repair the catalog before appending files"
                        .into(),
                );
            }
            if count == 1 {
                let batch = duplicate
                    .iter()
                    .find(|batch| batch.num_rows() == 1)
                    .ok_or("MCAP source row is missing")?;
                let data = batch
                    .column_by_name("payload")
                    .ok_or("MCAP source has no bytes")?;
                let previous = McapSources::bytes(data.as_ref(), 0)?;
                if previous != self.bytes {
                    return Err(
                        "Refusing to replace an immutable MCAP file with different bytes".into(),
                    );
                }
                already_imported = true;
            }
        }
        if !already_imported {
            connection
                .create_or_append_batches(McapDatasetDefinition::SOURCE_TABLE, vec![self.batch()?])
                .await?;
        }
        if !exists {
            relations.publish(connection).await?;
        } else {
            relations.append(connection).await?;
        }
        Ok(())
    }
}
impl StorageManager {
    pub(crate) async fn append_mcap_file(&self, file: McapFile<'_>) -> Result<(), String> {
        let exists = self.validate_mcap_destination(file.dataset_name).await?;
        let mut connection = self.project_store().connect_exclusive().await?;
        connection
            .transaction("MCAP file append", async |connection| {
                file.publish(connection, exists).await
            })
            .await
    }
}

/// Portable source files are feature data, provisioned by import rather than startup.
pub(crate) struct McapSources;
impl McapSources {
    pub(crate) fn bundle_schema() -> Schema {
        Schema::new(vec![
            arrow_schema::Field::new("dataset_name", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("recording", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("payload", arrow_schema::DataType::Binary, false),
        ])
    }
}
impl McapSources {
    fn bytes(array: &dyn arrow_array::Array, row: usize) -> Result<&[u8], String> {
        if array.is_null(row) {
            return Err("MCAP source payload is null".into());
        }
        if let Some(array) = array.as_any().downcast_ref::<arrow_array::BinaryArray>() {
            return Ok(array.value(row));
        }
        if let Some(array) = array.as_any().downcast_ref::<BinaryViewArray>() {
            return Ok(array.value(row));
        }
        if let Some(array) = array
            .as_any()
            .downcast_ref::<arrow_array::LargeBinaryArray>()
        {
            return Ok(array.value(row));
        }
        Err("MCAP source payload must be binary".into())
    }

    pub(crate) async fn validate_ownership(
        store: &crate::ProjectStore,
        connection: &mut crate::ProjectConnection,
    ) -> Result<Vec<String>, String> {
        let owned = store.managed_mcap_datasets().await?;
        if !connection.table_exists("dataset_source").await? {
            if !owned.is_empty() {
                return Err("MCAP datasets have no source table".into());
            }
            return Ok(owned);
        }
        let names = connection
            .query_rows("SELECT DISTINCT dataset_name FROM dataset_source", 100_000)
            .await?;
        if names.truncated() {
            return Err("Too many datasets to validate for project transfer".into());
        }
        for row in names.rows() {
            let name = row
                .get("dataset_name")
                .and_then(serde_json::Value::as_str)
                .ok_or("MCAP source has no dataset name")?;
            if !store.validate_mcap_destination(name).await? {
                return Err(format!(
                    "MCAP source {name} has no Dataset definition; repair the catalog before transferring the project"
                ));
            }
        }
        Ok(owned)
    }

    pub(crate) fn restore(batch: RecordBatch) -> Result<Vec<RecordBatch>, String> {
        let columns = crate::db::ArrowColumns::new(&batch, "MCAP source backup");
        let mut rows = Vec::new();
        let data = batch
            .column_by_name("payload")
            .ok_or("MCAP backup has no payload")?;
        for row in 0..batch.num_rows() {
            let name = columns.text("dataset_name", row)?;
            if !crate::db::StorageSchema::is_dataset_id(name) || name.contains('.') {
                return Err("MCAP backup contains an invalid dataset name".into());
            }
            let bytes = Self::bytes(data.as_ref(), row)?;
            rows.push(
                McapFile::new(
                    columns.text("dataset_name", row)?,
                    columns.text("recording", row)?,
                    bytes,
                )
                .batch()?,
            );
        }
        if rows.is_empty() {
            rows.push(McapFile::new("", "", &[]).batch()?.slice(0, 0));
        }
        Ok(rows)
    }
    pub(crate) async fn refresh(
        connection: &mut crate::ProjectConnection,
        names: &[String],
    ) -> Result<(), String> {
        let source_names = connection
            .query_rows("SELECT DISTINCT dataset_name FROM dataset_source", 100_000)
            .await?;
        if source_names.truncated()
            || source_names.rows().iter().any(|row| {
                row.get("dataset_name")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|name| !names.iter().any(|declared| declared == name))
            })
        {
            return Err("MCAP source names do not match the project manifest".into());
        }
        for name in names {
            let relations = McapDatasetDefinition::new(name);
            // Restore into a new project requires the source view and dataset definition.
            connection
                .execute_update_sql(&relations.source_view_sql())
                .await?;
            connection
                .execute_update_sql(&relations.create_sql())
                .await?;
            connection
                .execute_update_sql(&format!(
                    "REFRESH DATASET {}",
                    McapDatasetDefinition::quoted(&format!("dataset_{name}"))
                ))
                .await?;
            // A bundle carries no answers: the restored dataset starts over.
            relations.create_answers(connection).await?;
        }
        Ok(())
    }
}

impl McapSources {
    pub(crate) async fn retire(
        connection: &mut crate::ProjectConnection,
        names: &[String],
    ) -> Result<(), String> {
        for name in names {
            // Refreshing materializes each topic as a signal under the
            // Dataset, which only a cascading drop removes.
            connection
                .execute_update_sql(&format!(
                    "DROP DATASET IF EXISTS {} CASCADE",
                    McapDatasetDefinition::quoted(&format!("dataset_{name}"))
                ))
                .await?;
            connection
                .execute_update_sql(&format!(
                    "DROP VIEW IF EXISTS {}",
                    McapDatasetDefinition::quoted(&format!("dataset_source_{name}"))
                ))
                .await?;
            connection
                .execute_update_sql(&format!(
                    "DROP TABLE IF EXISTS {}",
                    AnswerTable::for_dataset_id(name).quoted()
                ))
                .await?;
        }
        Ok(())
    }
}

/// The relations Studio keeps for one MCAP dataset: its `dataset_<id>`
/// Dataset, the `dataset_source_<id>` view it reads its files through, and
/// the `dataset_answers_<id>` table of model answers about its episodes.
pub(crate) struct McapDatasetDefinition {
    dataset_name: String,
    dataset: String,
    source: String,
    answers: AnswerTable,
}
impl McapDatasetDefinition {
    pub(crate) const SOURCE_TABLE: &str = "dataset_source";

    pub(crate) fn new(dataset_id: &str) -> Self {
        Self {
            dataset_name: dataset_id.to_owned(),
            dataset: format!("dataset_{dataset_id}"),
            source: format!("dataset_source_{dataset_id}"),
            answers: AnswerTable::for_dataset_id(dataset_id),
        }
    }
    /// The Dataset's SQL name.
    pub(crate) fn dataset(&self) -> &str {
        &self.dataset
    }
    pub(crate) fn answers(&self) -> &AnswerTable {
        &self.answers
    }
    fn quoted(name: &str) -> String {
        name.split('.')
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".")
    }
    pub(crate) fn planned(&self) -> Vec<String> {
        vec![
            self.dataset.clone(),
            self.source.clone(),
            self.answers.name(),
        ]
    }
    pub(crate) fn create_sql(&self) -> String {
        format!(
            "CREATE DATASET {} USING spql.mcap.dataset FROM {} WITH (key = 'recording', blob = 'source', on_error = 'fail')",
            Self::quoted(&self.dataset),
            Self::quoted(&self.source)
        )
    }
    pub(crate) fn source_view_sql(&self) -> String {
        format!(
            "CREATE VIEW {} AS SELECT recording, source FROM {} WHERE dataset_name = {}",
            Self::quoted(&self.source),
            Self::SOURCE_TABLE,
            SqlLiteral::text(&self.dataset_name)
        )
    }
}

pub(crate) struct McapSourceBlob;

impl McapSourceBlob {
    fn fields() -> Fields {
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

    pub(crate) fn field(name: &str) -> Field {
        Field::new(name, DataType::Struct(Self::fields()), true).with_metadata(
            [(
                EXTENSION_TYPE_NAME_KEY.to_owned(),
                "spql.io.Blob".to_owned(),
            )]
            .into(),
        )
    }

    pub(crate) fn external_array(uris: &[String], sizes: &[u64]) -> Result<ArrayRef, String> {
        let len = sizes.len();
        if uris.len() != len {
            return Err("MCAP source URIs and sizes must have the same length".into());
        }
        let null_text = || Arc::new(StringViewArray::from(vec![None::<&str>; len])) as ArrayRef;
        let null_u64 = || Arc::new(UInt64Array::from(vec![None::<u64>; len])) as ArrayRef;
        let values = vec![
            Arc::new(BinaryViewArray::from(vec![None::<&[u8]>; len])) as ArrayRef,
            Arc::new(StringViewArray::from_iter_values(
                uris.iter().map(String::as_str),
            )) as ArrayRef,
            null_u64(),
            Arc::new(UInt64Array::from_iter_values(sizes.iter().copied())) as ArrayRef,
            Arc::new(StringViewArray::from_iter_values(
                sizes.iter().map(|_| "application/mcap"),
            )) as ArrayRef,
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
            null_text(),
        ];
        StructArray::try_new(Self::fields(), values, None)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("building Spiral external Blob array: {error}"))
    }
}

impl McapDatasetDefinition {
    /// Create the Dataset over its source view, refresh it, and create its
    /// answers table from what the refresh found.
    pub(crate) async fn publish(&self, connection: &mut ProjectConnection) -> Result<(), String> {
        connection
            .execute_update_sql(&self.source_view_sql())
            .await?;
        connection.execute_update_sql(&self.create_sql()).await?;
        self.refresh(connection).await?;
        self.create_answers(connection).await
    }

    /// Refresh the Dataset after an append, and give each episode it added a
    /// row of the answers table.
    async fn append(&self, connection: &mut ProjectConnection) -> Result<(), String> {
        self.refresh(connection).await?;
        self.add_answer_rows(connection).await
    }

    async fn refresh(&self, connection: &mut ProjectConnection) -> Result<(), String> {
        connection
            .execute_update_sql(&format!("REFRESH DATASET {}", Self::quoted(&self.dataset)))
            .await?;
        let members = connection
            .query_rows(
                &format!(
                    "SELECT COUNT(*) AS members FROM spql.mcap.episodes({})",
                    SqlLiteral::text(&self.dataset)
                ),
                1,
            )
            .await?;
        let sources = connection
            .query_rows(
                &format!(
                    "SELECT COUNT(*) AS members FROM dataset_source WHERE dataset_name = {}",
                    SqlLiteral::text(&self.dataset_name)
                ),
                1,
            )
            .await?;
        if members.rows() != sources.rows() {
            return Err("Dataset membership does not match its MCAP source files".into());
        }
        Ok(())
    }
}

impl crate::ProjectStore {
    pub(crate) async fn validate_mcap_destination(&self, name: &str) -> Result<bool, String> {
        if !crate::db::StorageSchema::is_dataset_id(name) || name.contains('.') {
            return Err("Dataset name must contain 1–96 letters, numbers, '_' or '-'".into());
        }
        let client = self.dataset_catalog();
        let resources = client.resources().await?;
        let dataset_name = format!("dataset_{name}");
        let source_name = format!("dataset_source_{name}");
        let Some(resource) = resources
            .iter()
            .find(|r| r.selection().label() == dataset_name)
        else {
            if resources
                .iter()
                .any(|r| r.selection().label() == source_name)
            {
                return Err("The MCAP source exists without its dataset; repair the catalog before appending files".into());
            }
            return Ok(false);
        };
        let dataset = resource
            .dataset()
            .ok_or("MCAP destination is not a Spiral dataset")?;
        let response: serde_json::Value = client
            .request(client.client.get(format!(
                "{}/datasets/{}/versions/{}",
                client.endpoint,
                dataset.resource_id(),
                dataset.version_id()
            )))
            .await?;
        let definition = &response["version"]["definition"];
        let payload: serde_json::Value = serde_json::from_str(
            definition["canonicalPayload"]
                .as_str()
                .ok_or("Dataset has no source definition")?,
        )
        .map_err(|e| e.to_string())?;
        if definition["producerId"] != "spql.mcap.dataset"
            || payload["sourceRelation"] != json!([source_name])
            || payload["options"]["key"] != "recording"
            || payload["options"]["blob"] != "source"
        {
            return Err("This dataset does not use Studio's managed MCAP source; choose a MCAP append destination with the shared source view".into());
        }
        Ok(true)
    }
}

impl crate::ProjectStore {
    pub(crate) async fn managed_mcap_datasets(&self) -> Result<Vec<String>, String> {
        let client = self.dataset_catalog();
        let resources = client.resources().await?;
        let mut names = Vec::new();
        for resource in &resources {
            if resource.dataset().is_none() {
                continue;
            }
            let label = resource.selection().label();
            let Some(name) = label.strip_prefix("dataset_") else {
                continue;
            };
            let source_name = format!("dataset_source_{name}");
            if resources
                .iter()
                .any(|resource| resource.selection().label() == source_name)
                && self.validate_mcap_destination(name).await?
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_restores_every_file_in_a_multirow_batch() {
        let batch = RecordBatch::try_new(
            Arc::new(McapSources::bundle_schema()),
            vec![
                Arc::new(StringArray::from(vec!["first", "second"])),
                Arc::new(StringArray::from(vec!["one.mcap", "two.mcap"])),
                Arc::new(arrow_array::BinaryArray::from(vec![
                    b"first file".as_slice(),
                    b"second file".as_slice(),
                ])),
            ],
        )
        .unwrap();
        let restored = McapSources::restore(batch).unwrap();
        assert_eq!(restored.len(), 2);
        for (batch, expected) in restored
            .iter()
            .zip([b"first file".as_slice(), b"second file".as_slice()])
        {
            let source = batch
                .column_by_name("source")
                .unwrap()
                .as_any()
                .downcast_ref::<StructArray>()
                .unwrap();
            assert_eq!(
                McapSources::bytes(source.column_by_name("bytes").unwrap().as_ref(), 0).unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires SPIRAL_MCAP_TEST_PROJECT and SPIRAL_ACCESS_TOKEN (endpoints from SPIRAL__SERVER__URL and SPIRAL__SPFS__URL); creates and removes a uniquely named dataset"]
    async fn mcap_files_share_publication_and_dataset_reads() {
        use crate::episodes::{CameraRecording, EpisodeFixture, McapRecording};
        let (store, storage) = crate::datasets::HostedProject::storage();
        let name = format!("source_probe_{}", uuid::Uuid::new_v4().simple());
        let other_name = format!("{name}_other");
        let cameras = ["wrist", "overhead"]
            .into_iter()
            .map(|stream| {
                CameraRecording::from_annex_b(
                    stream.into(),
                    (32, 32, 30),
                    (
                        include_bytes!(
                            "../../../../frontend/src/activities/episode/fixtures/seek.h264"
                        )
                        .to_vec(),
                        (0..8)
                            .map(|index| {
                                if stream == "overhead" {
                                    25_000_000 + index * 25_000_000
                                } else {
                                    index * 33_333_333
                                }
                            })
                            .collect(),
                    ),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let source_existed = store
            .connect()
            .await
            .unwrap()
            .table_exists("dataset_source")
            .await
            .unwrap();
        let outcome: Result<(), String> = async {
            for (id, camera_count) in [("first", 2), ("second", 1), ("third", 0)] {
                let id = format!("{name}_{id}");
                let episode = EpisodeFixture::new(&id, &name)
                    .with_arms(vec![spiral_robotics_common::recording::EpisodeArm::new(
                        "arm",
                        None,
                        vec!["joint".into()],
                    )])
                    .with_timing(1_700_000_000_000, 500.0)
                    .with_samples((0..120).map(|index| {
                        let sample = crate::episodes::Sample::new(index as f64 * 0.002, vec![vec![if index == 60 { None } else { Some(index as f64 / 120.0) }]], vec![vec![index]]);
                        if camera_count > 0 { sample.with_action(vec![Some(vec![Some(0.5)])]) } else { sample }
                    }).collect())
                    .with_duration_seconds(0.3)
                    .into_episode();
                let bytes = McapRecording::encode(&episode, &cameras[..camera_count])?;
                let key = format!("{id}.mcap");
                storage
                    .append_mcap_file(McapFile::new(&name, &key, &bytes))
                    .await?;
                storage
                    .append_mcap_file(McapFile::new(&name, &key, &bytes))
                    .await?;
            }
            // All topics exist in the dataset now; availability must still be exact per episode.
            let dataset = spiral_robotics_common::datasets::DatasetSelection::new(vec![format!("dataset_{name}")])?;
            let headers = store.connect().await?.query_rows(
                &format!("SELECT id, value.episode_id AS recording FROM spql.mcap.read_topic('dataset_{name}', '/studio/episode')"), 10,
            ).await?;
            assert_eq!(headers.rows().len(), 3);
            for row in headers.rows() {
                let member = spiral_robotics_common::datasets::EpisodeId::try_from(row["id"].as_str().unwrap()).unwrap();
                let recording = row["recording"].as_str().unwrap();
                let available = storage.episode_topics(
                    spiral_robotics_common::datasets::EpisodeSelection::new(dataset.clone(), member.clone())
                ).await?;
                let expected = if recording.ends_with("_first") {
                    vec!["/camera/overhead/video", "/camera/wrist/video"]
                } else if recording.ends_with("_second") {
                    vec!["/camera/wrist/video"]
                } else { vec![] };
                assert_eq!(available.iter().filter(|topic| topic.schemas().iter().any(|schema| schema == "foxglove.CompressedVideo")).map(|topic| topic.name()).collect::<Vec<_>>(), expected, "{recording}");
                assert!(available.iter().any(|topic| topic.name() == "/studio/episode"));
                assert!(available.iter().all(|topic| topic.members() == 1));
                let topic = available.iter().find(|topic| topic.name() == "/studio/joints/arm").unwrap();
                let read = spiral_robotics_common::datasets::MessageRead::schema(
                    spiral_robotics_common::datasets::EpisodeTopicRequest::new(dataset.clone(), topic.name().into(), member.clone()), topic.revision().unwrap().clone(),
                );
                let mut complete_messages = Vec::<serde_json::Value>::new();
                for fields in [vec![], vec![vec!["value".into(), "state_position".into(), "values".into()]]] {
                let projected = !fields.is_empty();
                let window = spiral_robotics_common::datasets::MessageWindow::new((1_700_000_000_000_000_000, 1_700_000_000_300_000_000), fields).with_limit(64);
                let mut next = None;
                let mut all = Vec::<serde_json::Value>::new();
                loop {
                    let page = MessagePage::read(&storage, &read.clone().with_window(window.clone().with_cursor(next))).await?;
                    next = page.next;
                    all.extend(page.rows);
                    if next.is_none() { break; }
                }
                assert_eq!(all.len(), 120, "paginated messages exceed the preview bound");
                assert_eq!(all[100]["log_time"].as_u64(), Some(1_700_000_000_200_000_000));
                if projected {
                    for (projected, complete) in all.iter().zip(&complete_messages) {
                        assert_eq!(projected["field_0"], complete["value"]["state_position"]["values"]);
                        assert_eq!(projected["log_time"], complete["log_time"]);
                    }
                    continue;
                }
                assert_eq!(all[100]["value"]["state_position"]["values"][0]["value"].as_f64(), Some(100.0 / 120.0));
                assert!(all[60]["value"]["state_position"]["values"][0]["value"].is_null());
                assert_eq!(all[0]["value"]["action_valid"].as_bool(), Some(!recording.ends_with("_third")));
                complete_messages = all;
                }

            }
            assert!(storage.episode_topics(
                spiral_robotics_common::datasets::EpisodeSelection::new(dataset, spiral_robotics_common::datasets::EpisodeId::try_from(uuid::Uuid::new_v4().simple().to_string()).unwrap())
            ).await.unwrap_err().contains("no longer in the dataset"));
            // A foreign producer needs no Studio DTO or topic-name convention.
            let bytes = SensorFixture::encode()?;
            storage.append_mcap_file(McapFile::new(&name, "arbitrary/nested-key", &bytes)).await?;
            storage.append_mcap_file(McapFile::new(&name, "arbitrary/nested-key", &bytes)).await?;
            // The same source key in another dataset is independent.
            storage.append_mcap_file(McapFile::new(&other_name, "arbitrary/nested-key", &bytes)).await?;
            let other_members = store.connect().await?.query_rows(&format!("SELECT id FROM spql.mcap.episodes('dataset_{other_name}')"), 2).await?;
            assert_eq!(other_members.rows().len(), 1);
            let members = store.connect().await?.query_rows(&format!("SELECT id FROM spql.mcap.read_topic('dataset_{name}', '/environment') LIMIT 1"), 1).await?;
            let member = spiral_robotics_common::datasets::EpisodeId::try_from(members.rows()[0]["id"].as_str().unwrap()).unwrap();
            let dataset = spiral_robotics_common::datasets::DatasetSelection::new(vec![format!("dataset_{name}")])?;
            let topics = storage.episode_topics(spiral_robotics_common::datasets::EpisodeSelection::new(dataset.clone(), member.clone())).await?;
            let topic = topics.iter().find(|topic| topic.name() == "/environment").unwrap();
            let request = spiral_robotics_common::datasets::MessageRead::schema(
                spiral_robotics_common::datasets::EpisodeTopicRequest::new(dataset, topic.name().into(), member), topic.revision().unwrap().clone());
            let schema = MessagePage::read(&storage, &request).await?;
            assert!(schema.schema.field_with_name("value").is_ok());
            assert!(schema.rows.is_empty());
            let mut cursor = None;
            let window = spiral_robotics_common::datasets::MessageWindow::new((100, 101), vec![vec!["value".into(), "counter".into()]]).with_limit(1);
            for expected in [u64::MAX - 1, u64::MAX] {
                let page = MessagePage::read(&storage, &request.clone().with_window(window.clone().with_cursor(cursor))).await?;
                cursor = page.next;
                assert_eq!(page.rows.len(), 1);
                assert_eq!(page.rows[0]["field_0"].as_u64(), Some(expected));
            }
            assert!(cursor.is_none());

            Ok(())
        }
        .await;
        for probe in [&name, &other_name] {
            storage.drop_probe_dataset(probe).await.unwrap();
        }
        let mut connection = store.connect_exclusive().await.unwrap();
        if !source_existed && connection.table_exists("dataset_source").await.unwrap() {
            connection
                .execute_update_sql("DROP TABLE dataset_source")
                .await
                .unwrap();
        }
        outcome.expect("MCAP source boundary");
    }
    /// One page of a message read, fetched the way the browser pages: one
    /// Arrow episode query, continuing after the last kept row.
    struct MessagePage {
        schema: Arc<Schema>,
        rows: Vec<serde_json::Value>,
        next: Option<spiral_robotics_common::datasets::MessageCursor>,
    }
    impl MessagePage {
        async fn read(
            storage: &StorageManager,
            read: &spiral_robotics_common::datasets::MessageRead,
        ) -> Result<Self, String> {
            let request = spiral_robotics_common::relations::EpisodeQueryRequest::new(
                read.episode(),
                read.sql()?,
                read.max_rows(),
            );
            let bytes = storage.queries().arrow_for_episode(&request).await?;
            let reader =
                arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
                    .map_err(|e| e.to_string())?;
            let schema = reader.schema();
            let batches = reader
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let mut rows = Vec::<serde_json::Value>::new();
            if !batches.is_empty() {
                let mut json = Vec::new();
                let mut writer = arrow_json::writer::WriterBuilder::new()
                    .with_explicit_nulls(true)
                    .build::<_, arrow_json::writer::JsonArray>(&mut json);
                writer
                    .write_batches(&batches.iter().collect::<Vec<_>>())
                    .map_err(|e| e.to_string())?;
                writer.finish().map_err(|e| e.to_string())?;
                drop(writer);
                rows = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
            }
            let limit = read.window().map_or(0, |window| window.limit());
            let next = if rows.len() > limit {
                let last = &rows[limit - 1];
                let position =
                    |name: &str| last[name].as_u64().ok_or_else(|| format!("Missing {name}"));
                Some(spiral_robotics_common::datasets::MessageCursor::new(
                    position("log_time")?,
                    position("chunk_start_offset")?,
                    u32::try_from(position("sample_ordinal")?).map_err(|e| e.to_string())?,
                ))
            } else {
                None
            };
            rows.truncate(limit);
            Ok(Self { schema, rows, next })
        }
    }

    struct SensorFixture;
    impl SensorFixture {
        fn encode() -> Result<Vec<u8>, String> {
            use prost::Message;
            use prost_reflect::{
                DescriptorPool, DynamicMessage, Value,
                prost_types::{
                    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
                    field_descriptor_proto::{Label, Type},
                },
            };
            let descriptor = FileDescriptorSet {
                file: vec![FileDescriptorProto {
                    name: Some("environment.proto".into()),
                    package: Some("example".into()),
                    syntax: Some("proto2".into()),
                    message_type: vec![DescriptorProto {
                        name: Some("Environment".into()),
                        field: vec![
                            FieldDescriptorProto {
                                name: Some("temperature".into()),
                                number: Some(1),
                                label: Some(Label::Optional as i32),
                                r#type: Some(Type::Double as i32),
                                ..Default::default()
                            },
                            FieldDescriptorProto {
                                name: Some("counter".into()),
                                number: Some(2),
                                label: Some(Label::Optional as i32),
                                r#type: Some(Type::Uint64 as i32),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }
            .encode_to_vec();
            let pool = DescriptorPool::decode(descriptor.as_slice()).map_err(|e| e.to_string())?;
            let mut writer =
                mcap::Writer::new(std::io::Cursor::new(Vec::new())).map_err(|e| e.to_string())?;
            let schema = writer
                .add_schema("example.Environment", "protobuf", &descriptor)
                .map_err(|e| e.to_string())?;
            let channel_id = writer
                .add_channel(schema, "/environment", "protobuf", &Default::default())
                .map_err(|e| e.to_string())?;
            for sequence in 0..2 {
                let mut message =
                    DynamicMessage::new(pool.get_message_by_name("example.Environment").unwrap());
                message
                    .set_field_by_name("counter", Value::U64(u64::MAX - 1 + u64::from(sequence)));
                if sequence == 0 {
                    message.set_field_by_name("temperature", Value::F64(23.5));
                }
                writer
                    .write_to_known_channel(
                        &mcap::records::MessageHeader {
                            channel_id,
                            sequence,
                            log_time: 100,
                            publish_time: 100,
                        },
                        &message.encode_to_vec(),
                    )
                    .map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
            }
            writer.finish().map_err(|e| e.to_string())?;
            Ok(writer.into_inner().into_inner())
        }
    }
}
