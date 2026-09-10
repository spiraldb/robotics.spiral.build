//! Bounded Arrow IPC pages from native MCAP observations; no payload-specific DTOs.
use super::{DatasetCatalogClient, EpisodeTopicQuery};
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use arrow_array::RecordBatch;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Schema};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{MessageCursor, MessageRead};
use std::{collections::HashMap, sync::Arc};

impl StorageManager {
    pub async fn episode_messages(&self, request: MessageRead) -> Result<Vec<u8>, String> {
        let catalog = DatasetCatalogClient::connect(self.connection_settings()).await?;
        let before = catalog.inspect(request.topic().dataset().clone()).await?;
        if before.dataset().version_id() != request.revision().version() {
            return Err("Dataset revision changed; reopen this episode".into());
        }
        if !before.topics().iter().any(|topic| {
            topic.name() == request.topic().topic() && topic.revision() == Some(request.revision())
        }) {
            return Err("This topic/schema revision no longer exists".into());
        }
        let query = MessageQuery { request: &request };
        let mut connection = self.project_store().connect().await?;
        let schema_stream = connection
            .query(query.statement(
                "log_time, publish_time, channel_id, chunk_start_offset, sample_ordinal, value",
                "LIMIT 0",
            )?)
            .await
            .map_err(|e| e.to_string())?;
        let schema = schema_stream.schema().clone();
        drop(schema_stream);
        let mut page = MessagePage::new(schema);
        if let Some(window) = request.window() {
            let projection = query.projection(&page.schema)?;
            let mut stream = connection
                .query(query.statement(&projection, &query.window_clause()?)?)
                .await
                .map_err(|e| e.to_string())?;
            page = MessagePage::new(stream.schema().clone());
            while let Some(batch) = stream.next().await {
                if !page.append(batch.map_err(|e| e.to_string())?, window.limit())? {
                    break;
                }
            }
        }
        // Do not deliver a page whose query raced with a dataset replacement.
        if catalog
            .inspect(request.topic().dataset().clone())
            .await?
            .dataset()
            .version_id()
            != request.revision().version()
        {
            return Err("Dataset revision changed during the read; reopen this episode".into());
        }
        page.encode(&request)
    }
}
struct MessageQuery<'a> {
    request: &'a MessageRead,
}
impl MessageQuery<'_> {
    fn statement(&self, fields: &str, tail: &str) -> Result<crate::db::SqlStatement, String> {
        let topic = self.request.topic();
        let sql = format!(
            "SELECT {fields} FROM spql.mcap.read_topic('{}', '{}', '{}') WHERE id = $1 {tail}",
            topic.dataset().sql_name().replace('\'', "''"),
            topic.topic().replace('\'', "''"),
            self.request.revision().variant().replace('\'', "''")
        );
        Ok(EpisodeTopicQuery::for_sql(topic, sql)?.statement)
    }
    fn projection(&self, schema: &Schema) -> Result<String, String> {
        let mut fields = vec![
            "log_time".to_owned(),
            "publish_time".into(),
            "channel_id".into(),
            "chunk_start_offset".into(),
            "sample_ordinal".into(),
        ];
        let requested = self.request.window().ok_or("Missing window")?.fields();
        if requested.is_empty() {
            fields.push("value".into());
        }
        for (index, path) in requested.iter().enumerate() {
            if path.first().map(String::as_str) != Some("value") || path.len() > 16 {
                return Err("Fields must be paths in the message value".into());
            }
            let mut data_type = DataType::Struct(schema.fields().clone());
            for name in path {
                let DataType::Struct(children) = &data_type else {
                    return Err("Project the containing list to inspect its elements".into());
                };
                data_type = children
                    .iter()
                    .find(|field| field.name() == name)
                    .ok_or("Field is absent from the message schema")?
                    .data_type()
                    .clone();
            }
            fields.push(format!(
                "{} AS field_{index}",
                path.iter()
                    .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(".")
            ));
        }
        Ok(fields.join(", "))
    }
    fn window_clause(&self) -> Result<String, String> {
        let window = self.request.window().ok_or("Missing window")?;
        let (start, end) = window.bounds()?;
        let after = match window.after() {
            Some(cursor) => {
                let time = cursor.time()?;
                let chunk = cursor.chunk()?;
                format!(
                    " AND (log_time > {time} OR (log_time = {time} AND (chunk_start_offset > {chunk} OR (chunk_start_offset = {chunk} AND sample_ordinal > {}))))",
                    cursor.ordinal()
                )
            }
            None => String::new(),
        };
        Ok(format!(
            "AND log_time >= {start} AND log_time < {end}{after} ORDER BY log_time, chunk_start_offset, sample_ordinal LIMIT {}",
            window.limit() + 1
        ))
    }
}
struct MessagePage {
    schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
    rows: usize,
    bytes: usize,
    more: bool,
}
impl MessagePage {
    fn new(schema: Arc<Schema>) -> Self {
        Self {
            schema,
            batches: vec![],
            rows: 0,
            bytes: 0,
            more: false,
        }
    }
    fn append(&mut self, batch: RecordBatch, limit: usize) -> Result<bool, String> {
        let count = batch.num_rows().min(limit.saturating_sub(self.rows));
        if count < batch.num_rows() {
            self.more = true;
        }
        if count > 0 {
            let batch = batch.slice(0, count);
            self.bytes += batch.get_array_memory_size();
            if self.bytes > 32 * 1024 * 1024 {
                return Err(
                    "Message page exceeds 32 MiB; select fewer fields or a smaller window".into(),
                );
            }
            self.rows += count;
            self.batches.push(batch);
        }
        Ok(!self.more)
    }
    fn encode(self, request: &MessageRead) -> Result<Vec<u8>, String> {
        let next = if self.more {
            let batch = self
                .batches
                .last()
                .ok_or("A message exceeds the page limit")?;
            let row = batch.num_rows() - 1;
            let columns = ArrowColumns::new(batch, "message cursor");
            Some(MessageCursor::new(
                columns.unsigned("log_time", row)?,
                columns.unsigned("chunk_start_offset", row)?,
                u32::try_from(columns.unsigned("sample_ordinal", row)?)
                    .map_err(|e| e.to_string())?,
            ))
        } else {
            None
        };
        let mut metadata: HashMap<_, _> = self.schema.metadata().clone();
        metadata.insert(
            "studio.message_read".into(),
            serde_json::to_string(request).map_err(|e| e.to_string())?,
        );
        metadata.insert(
            "studio.next_cursor".into(),
            serde_json::to_string(&next).map_err(|e| e.to_string())?,
        );
        let schema = self.schema.as_ref().clone().with_metadata(metadata);
        let mut bytes = Vec::new();
        let mut writer = StreamWriter::try_new(&mut bytes, &schema).map_err(|e| e.to_string())?;
        for batch in self.batches {
            writer.write(&batch).map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
        drop(writer);
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(
                "Encoded message page exceeds 32 MiB; select fewer fields or a smaller window"
                    .into(),
            );
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, UInt32Array, UInt64Array};
    use arrow_schema::Field;
    use spiral_robotics_common::datasets::{
        DatasetSelection, EpisodeTopicRequest, MessageWindow, TopicRevision,
    };

    fn request() -> MessageRead {
        MessageRead::schema(
            EpisodeTopicRequest::new(
                DatasetSelection::new(vec!["dataset_test".into()]).unwrap(),
                "/sensor".into(),
                uuid::Uuid::nil().to_string(),
            ),
            TopicRevision::new("version".into(), "variant".into()),
        )
    }

    #[test]
    fn arrow_pages_preserve_exact_values_and_tied_timestamp_cursor() {
        let batch = RecordBatch::try_from_iter(vec![
            (
                "log_time",
                Arc::new(UInt64Array::from(vec![u64::MAX; 3])) as ArrayRef,
            ),
            ("sample_ordinal", Arc::new(UInt32Array::from(vec![4, 5, 6]))),
            (
                "field_0",
                Arc::new(UInt64Array::from(vec![Some(u64::MAX), None, Some(1)])),
            ),
            (
                "chunk_start_offset",
                Arc::new(UInt64Array::from(vec![100, 100, 200])),
            ),
        ])
        .unwrap();
        let read = request().with_window(MessageWindow::new((0, u64::MAX), vec![]).with_limit(2));
        let mut page = MessagePage::new(batch.schema());
        assert!(!page.append(batch, 2).unwrap());
        let reader = arrow_ipc::reader::StreamReader::try_new(
            std::io::Cursor::new(page.encode(&read).unwrap()),
            None,
        )
        .unwrap();
        let cursor: MessageCursor =
            serde_json::from_str(&reader.schema().metadata()["studio.next_cursor"]).unwrap();
        assert_eq!(cursor.time().unwrap(), u64::MAX);
        assert_eq!(cursor.ordinal(), 5);
        assert_eq!(cursor.chunk().unwrap(), 100);
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches[0].num_rows(), 2);
        assert_eq!(
            batches[0]
                .column(2)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0),
            u64::MAX
        );
    }

    #[test]
    fn projections_validate_struct_paths_and_quote_literal_field_names() {
        let schema = Schema::new(vec![Field::new(
            "value",
            DataType::Struct(vec![Arc::new(Field::new("a.b\"c", DataType::Float64, true))].into()),
            false,
        )]);
        let read = request().with_window(MessageWindow::new(
            (1, 10),
            vec![vec!["value".into(), "a.b\"c".into()]],
        ));
        let query = MessageQuery { request: &read };
        assert!(
            query
                .projection(&schema)
                .unwrap()
                .ends_with("\"value\".\"a.b\"\"c\" AS field_0")
        );
        assert!(
            query
                .window_clause()
                .unwrap()
                .contains("log_time >= 1 AND log_time < 10")
        );
        let invalid = request().with_window(MessageWindow::new(
            (1, 10),
            vec![vec!["value".into(), "absent".into()]],
        ));
        assert!(
            MessageQuery { request: &invalid }
                .projection(&schema)
                .is_err()
        );
        assert!(MessageWindow::new((10, 1), vec![]).bounds().is_err());
    }
}
