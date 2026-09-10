//! Publish one complete MCAP file through the Dataset's declared source view.
use crate::{
    StorageManager,
    db::SpiralSql,
    transfer::{ImportBlobStorage, ImportRelations},
};
use arrow_array::{
    ArrayRef, BinaryViewArray, RecordBatch, StringArray, StringViewArray, StructArray, UInt64Array,
};
use arrow_schema::Schema;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

pub(crate) struct RecordingImport<'a> {
    dataset_name: &'a str,
    recording: &'a str,
    bytes: &'a [u8],
}
impl<'a> RecordingImport<'a> {
    pub(crate) fn new(dataset_name: &'a str, recording: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            dataset_name,
            recording,
            bytes,
        }
    }
    fn batch(&self) -> Result<RecordBatch, String> {
        let text = || Arc::new(StringViewArray::from(vec![None::<&str>])) as ArrayRef;
        let schema = Arc::new(Schema::new(vec![
            arrow_schema::Field::new("dataset_name", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("recording", arrow_schema::DataType::Utf8, false),
            ImportBlobStorage::field("source"),
        ]));
        let hash = format!("{:x}", Sha256::digest(self.bytes));
        let blob = StructArray::try_new(
            ImportBlobStorage::fields(),
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
                Arc::new(StringArray::from(vec![self.recording])),
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
        let relations = ImportRelations::new(self.dataset_name);
        let mut already_imported = false;
        if connection
            .table_exists(ImportRelations::SOURCE_TABLE)
            .await?
        {
            if !exists {
                let orphaned = connection
                    .query_rows(
                        &format!(
                            "SELECT recording FROM dataset_source WHERE dataset_name = {} LIMIT 1",
                            SpiralSql::string_literal(self.dataset_name)
                        ),
                        1,
                    )
                    .await?;
                if !orphaned.rows().is_empty() {
                    return Err("Recording sources already exist without their Dataset; repair the catalog before recording".into());
                }
            }
            let duplicate = connection.read_batches(&format!(
                "SELECT spql.io.read_bytes(source) AS payload FROM dataset_source WHERE dataset_name = {} AND recording = {} LIMIT 2",
                SpiralSql::string_literal(self.dataset_name), SpiralSql::string_literal(self.recording)
            )).await?;
            let count = duplicate.iter().map(RecordBatch::num_rows).sum::<usize>();
            if count > 1 {
                return Err(
                    "Duplicate recording sources exist; repair the catalog before recording".into(),
                );
            }
            if count == 1 {
                let batch = duplicate
                    .iter()
                    .find(|batch| batch.num_rows() == 1)
                    .ok_or("Recording source row is missing")?;
                let data = batch
                    .column_by_name("payload")
                    .ok_or("Recording source has no bytes")?;
                let previous = McapSources::bytes(data.as_ref(), 0)?;
                if previous != self.bytes {
                    return Err(
                        "Refusing to replace an immutable MCAP recording with different bytes"
                            .into(),
                    );
                }
                already_imported = true;
            }
        }
        if !already_imported {
            connection
                .create_or_append_batches(ImportRelations::SOURCE_TABLE, vec![self.batch()?])
                .await?;
        }
        if !exists {
            connection
                .execute_update_sql(&relations.source_view_sql())
                .await?;
            connection
                .execute_update_sql(&relations.create_sql())
                .await?;
        }
        connection
            .execute_update_sql(&format!(
                "REFRESH DATASET {}",
                ImportRelations::quoted(&format!("dataset_{}", self.dataset_name))
            ))
            .await?;
        let episode = self
            .recording
            .strip_suffix(".mcap")
            .ok_or("Recording filename must end in .mcap")?;
        let visible = connection.query_rows(&format!("SELECT id FROM spql.mcap.read_topic({}, '/studio/episode') WHERE value.episode_id = {} LIMIT 2", SpiralSql::string_literal(&format!("dataset_{}", self.dataset_name)), SpiralSql::string_literal(episode)), 2).await?;
        if visible.rows().len() != 1 {
            return Err("MCAP import did not publish exactly one matching episode".into());
        }
        Ok(())
    }
}
impl StorageManager {
    pub async fn recorded_camera(
        &self,
        id: &str,
        stream: &str,
    ) -> Result<spiral_robotics_common::datasets::EpisodeVideo, String> {
        let mut connection = self.project_store().connect().await?;
        let sources = connection
            .query_rows(
                &format!(
                    "SELECT dataset_name FROM dataset_source WHERE recording = {} LIMIT 2",
                    SpiralSql::string_literal(&format!("{id}.mcap"))
                ),
                2,
            )
            .await?;
        if sources.rows().len() != 1 {
            return Err("Recording source is absent or ambiguous across datasets".into());
        }
        let name = sources.rows()[0]
            .get("dataset_name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Recording dataset name is missing")?;
        let dataset = spiral_robotics_common::datasets::DatasetSelection::new(vec![format!(
            "dataset_{name}"
        )])?;
        let headers = connection.query_rows(&format!("SELECT id, value.started_ms AS started_ms FROM spql.mcap.read_topic({}, '/studio/episode') WHERE value.episode_id = {} LIMIT 2", SpiralSql::string_literal(&dataset.sql_name()), SpiralSql::string_literal(id)), 2).await?;
        if headers.rows().len() != 1 {
            return Err("Recording header is absent or ambiguous in its Dataset".into());
        }
        let header = &headers.rows()[0];
        let member = header
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Recording member ID is missing")?;
        let origin = header
            .get("started_ms")
            .and_then(serde_json::Value::as_u64)
            .and_then(|time| time.checked_mul(1_000_000))
            .ok_or("Recording start time is invalid")?;
        let request = spiral_robotics_common::datasets::EpisodeTopicRequest::new(
            dataset,
            format!("/camera/{stream}/video"),
            member.to_owned(),
        );
        drop(connection);
        self.episode_video(request).await?.relative_to(origin)
    }

    pub(crate) async fn import_recording(
        &self,
        recording: RecordingImport<'_>,
    ) -> Result<(), String> {
        let exists = self
            .validate_recording_destination(recording.dataset_name)
            .await?;
        let mut connection = self.project_store().connect_exclusive().await?;
        connection.begin_transaction().await?;
        let result = recording.publish(&mut connection, exists).await;
        connection
            .finish_transaction(result, "MCAP recording import")
            .await
    }
}

/// Portable source files are feature data, provisioned by import rather than startup.
pub(crate) struct McapSources;
impl crate::db::ManagedRelationSchema for McapSources {
    fn relations(&self) -> &'static [crate::db::Table] {
        &[crate::db::Table::RecordingSources]
    }
    fn managed_relation_schema(&self, table: crate::db::Table) -> Option<Schema> {
        (table == crate::db::Table::RecordingSources).then(|| {
            Schema::new(vec![
                arrow_schema::Field::new("dataset_name", arrow_schema::DataType::Utf8, false),
                arrow_schema::Field::new("recording", arrow_schema::DataType::Utf8, false),
                arrow_schema::Field::new("payload", arrow_schema::DataType::Binary, false),
            ])
        })
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
        let owned = store.recording_dataset_names().await?;
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
            if !store.validate_recording_destination(name).await? {
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
                RecordingImport::new(
                    columns.text("dataset_name", row)?,
                    columns.text("recording", row)?,
                    bytes,
                )
                .batch()?,
            );
        }
        if rows.is_empty() {
            rows.push(RecordingImport::new("", "", &[]).batch()?.slice(0, 0));
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
            let relations = ImportRelations::new(name);
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
                    ImportRelations::quoted(&format!("dataset_{name}"))
                ))
                .await?;
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
            connection
                .execute_update_sql(&format!(
                    "DROP DATASET IF EXISTS {}",
                    ImportRelations::quoted(&format!("dataset_{name}"))
                ))
                .await?;
            connection
                .execute_update_sql(&format!(
                    "DROP VIEW IF EXISTS {}",
                    ImportRelations::quoted(&format!("dataset_source_{name}"))
                ))
                .await?;
        }
        Ok(())
    }
}

impl StorageManager {
    pub(crate) async fn delete_recording(&self, id: &str) -> Result<(), String> {
        let key = SpiralSql::string_literal(&format!("{id}.mcap"));
        let mut connection = self.project_store().connect_exclusive().await?;
        let rows = connection
            .query_rows(
                &format!("SELECT dataset_name FROM dataset_source WHERE recording = {key} LIMIT 2"),
                2,
            )
            .await?;
        if rows.rows().len() != 1 {
            return Err("Recording source is absent or ambiguous across datasets".into());
        }
        let name = rows.rows()[0]
            .get("dataset_name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Recording source has no dataset")?;
        self.validate_recording_destination(name).await?;
        connection.begin_transaction().await?;
        let mutation = async {
            connection
                .execute_update_sql(&format!(
                    "DELETE FROM dataset_source WHERE recording = {key} AND dataset_name = {}",
                    SpiralSql::string_literal(name)
                ))
                .await?;
            connection
                .execute_update_sql(&format!(
                    "REFRESH DATASET {}",
                    ImportRelations::quoted(&format!("dataset_{name}"))
                ))
                .await?;
            Ok(())
        }
        .await;
        connection
            .finish_transaction(mutation, "recording deletion")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_restores_every_file_in_a_multirow_batch() {
        use crate::db::ManagedRelationSchema as _;
        let batch = RecordBatch::try_new(
            Arc::new(
                McapSources
                    .managed_relation_schema(crate::db::Table::RecordingSources)
                    .unwrap(),
            ),
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
    #[ignore = "requires SPIRAL_MCAP_TEST_CONFIG and Spiral access; creates and removes a uniquely named dataset"]
    async fn native_recording_import_retry_playback_and_delete() {
        use crate::episodes::{CameraRecording, EpisodeFixture, McapRecording};
        use crate::settings::{BackendStateStore, SpiralConnectionSettings};
        let config = BackendStateStore::open_for_test(
            std::env::var_os("SPIRAL_MCAP_TEST_CONFIG")
                .expect("explicit test config")
                .into(),
        )
        .unwrap();
        let settings = SpiralConnectionSettings::new(
            config.spiral_endpoint().unwrap(),
            config.spiral_spfs_endpoint().unwrap(),
        )
        .unwrap()
        .with_project(&config.project_id().unwrap())
        .unwrap()
        .with_credentials(config.credentials());
        let store = Arc::new(
            crate::ProjectStore::new(
                settings.clone(),
                "mcap_probe".into(),
                Arc::new(crate::AdbcDatabase::new(settings)),
            )
            .unwrap(),
        );
        let storage = StorageManager::new(store.clone());
        let name = format!("recording_probe_{}", uuid::Uuid::new_v4().simple());
        let cameras = ["wrist", "overhead"]
            .into_iter()
            .map(|stream| {
                CameraRecording::from_annex_b(
                    stream.into(),
                    (32, 32, 30),
                    (
                        include_bytes!(
                            "../../../../frontend/src/activities/data/fixtures/seek.h264"
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
                    .import_recording(RecordingImport::new(&name, &key, &bytes))
                    .await?;
                storage
                    .import_recording(RecordingImport::new(&name, &key, &bytes))
                    .await?;
                if camera_count == 0 { continue; }
                let video = storage.recorded_camera(&id, "wrist").await?;
                assert_eq!(video.packets().len(), 8);
                assert_eq!(video.packets()[0].timestamp_ns(), "0");
                assert!(video.packets()[0].keyframe());
            }
            // All topics exist in the dataset now; availability must still be exact per episode.
            let dataset = spiral_robotics_common::datasets::DatasetSelection::new(vec![format!("dataset_{name}")])?;
            let headers = store.connect().await?.query_rows(
                &format!("SELECT id, value.episode_id AS recording FROM spql.mcap.read_topic('dataset_{name}', '/studio/episode')"), 10,
            ).await?;
            assert_eq!(headers.rows().len(), 3);
            for row in headers.rows() {
                let member = row["id"].as_str().unwrap().to_owned();
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
                    let bytes = storage.episode_messages(read.clone().with_window(window.clone().with_cursor(next))).await?;
                    let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).map_err(|e| e.to_string())?;
                    next = serde_json::from_str(reader.schema().metadata().get("studio.next_cursor").unwrap()).map_err(|e| e.to_string())?;
                    let batches = reader.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
                    let mut json = Vec::new();
                    let mut writer = arrow_json::writer::WriterBuilder::new().with_explicit_nulls(true).build::<_, arrow_json::writer::JsonArray>(&mut json);
                    writer.write_batches(&batches.iter().collect::<Vec<_>>()).map_err(|e| e.to_string())?;
                    writer.finish().map_err(|e| e.to_string())?; drop(writer);
                    all.extend(serde_json::from_slice::<Vec<serde_json::Value>>(&json).map_err(|e| e.to_string())?);
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

                for topic in available.into_iter().filter(|topic| topic.schemas().iter().any(|schema| schema == "foxglove.CompressedVideo")) {
                    let video = storage.episode_video(spiral_robotics_common::datasets::EpisodeTopicRequest::new(
                        dataset.clone(), topic.name().to_owned(), member.clone()
                    )).await?;
                    assert_eq!(video.packets().len(), 8);
                    let offset = if topic.name().contains("overhead") { 25_000_000 } else { 0 };
                    assert_eq!(video.packets()[0].timestamp_ns().parse::<u64>().unwrap(), 1_700_000_000_000_000_000 + offset);
                }
            }
            assert!(storage.episode_topics(
                spiral_robotics_common::datasets::EpisodeSelection::new(dataset, uuid::Uuid::new_v4().to_string())
            ).await.unwrap_err().contains("no longer in the dataset"));
            storage.delete_recording(&format!("{name}_first")).await?;
            let mut connection = store.connect().await?;
            let rows = connection
                .query_rows(
                    &format!("SELECT id FROM spql.mcap.episodes('dataset_{name}')"),
                    10,
                )
                .await?;
            assert_eq!(rows.rows().len(), 2);
            drop(connection);
            // A foreign producer needs no Studio DTO or topic-name convention.
            let bytes = SensorFixture::encode()?;
            let mut connection = store.connect_exclusive().await?;
            connection.create_or_append_batches(ImportRelations::SOURCE_TABLE, vec![RecordingImport::new(&name, "foreign-sensor.mcap", &bytes).batch()?]).await?;
            connection.execute_update_sql(&format!("REFRESH DATASET dataset_{name}")).await?;
            drop(connection);
            let members = store.connect().await?.query_rows(&format!("SELECT id FROM spql.mcap.read_topic('dataset_{name}', '/environment') LIMIT 1"), 1).await?;
            let member = members.rows()[0]["id"].as_str().unwrap().to_owned();
            let dataset = spiral_robotics_common::datasets::DatasetSelection::new(vec![format!("dataset_{name}")])?;
            let topics = storage.episode_topics(spiral_robotics_common::datasets::EpisodeSelection::new(dataset.clone(), member.clone())).await?;
            let topic = topics.iter().find(|topic| topic.name() == "/environment").unwrap();
            let request = spiral_robotics_common::datasets::MessageRead::schema(
                spiral_robotics_common::datasets::EpisodeTopicRequest::new(dataset, topic.name().into(), member), topic.revision().unwrap().clone());
            let schema_bytes = storage.episode_messages(request.clone()).await?;
            let schema_reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(schema_bytes), None).map_err(|e| e.to_string())?;
            assert!(schema_reader.schema().field_with_name("value").is_ok());
            let mut cursor = None;
            let window = spiral_robotics_common::datasets::MessageWindow::new((100, 101), vec![vec!["value".into(), "counter".into()]]).with_limit(1);
            for expected in [u64::MAX - 1, u64::MAX] {
                let bytes = storage.episode_messages(request.clone().with_window(window.clone().with_cursor(cursor))).await?;
                let reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).map_err(|e| e.to_string())?;
                cursor = serde_json::from_str(&reader.schema().metadata()["studio.next_cursor"]).map_err(|e| e.to_string())?;
                let batches = reader.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
                assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
                let batch = batches.iter().find(|batch| batch.num_rows() > 0).unwrap();
                assert_eq!(crate::db::ArrowColumns::new(batch, "sensor").unsigned("field_0", 0)?, expected);
            }
            assert!(cursor.is_none());

            Ok(())
        }
        .await;
        let mut connection = store.connect_exclusive().await.unwrap();
        connection
            .execute_update_sql(&format!("DROP DATASET IF EXISTS dataset_{name}"))
            .await
            .unwrap();
        connection
            .execute_update_sql(&format!("DROP VIEW IF EXISTS dataset_source_{name}"))
            .await
            .unwrap();
        if connection.table_exists("dataset_source").await.unwrap() {
            connection
                .execute_update_sql(&format!(
                    "DELETE FROM dataset_source WHERE dataset_name = '{name}'"
                ))
                .await
                .unwrap();
            if !source_existed {
                connection
                    .execute_update_sql("DROP TABLE dataset_source")
                    .await
                    .unwrap();
            }
        }
        outcome.expect("native MCAP recording pipeline");
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
