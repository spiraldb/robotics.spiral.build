//! Schema-neutral reads of one immutable native topic revision.
//!
//! A read is one episode query: `$1` binds the episode UUID, and a windowed
//! read pages by keyset on `(log_time, chunk_start_offset, sample_ordinal)`.
use super::{EpisodeSelection, EpisodeTopicRequest};
use crate::sql::SqlLiteral;
use serde::{Deserialize, Serialize};

/// Every message's native position; a page continues after the last one.
const MESSAGE_POSITION: &str =
    "log_time, publish_time, channel_id, chunk_start_offset, sample_ordinal";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicRevision {
    variant: String,
}
impl TopicRevision {
    pub fn new(variant: String) -> Self {
        Self { variant }
    }
    pub fn variant(&self) -> &str {
        &self.variant
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRead {
    topic: EpisodeTopicRequest,
    revision: TopicRevision,
    window: Option<MessageWindow>,
}
impl MessageRead {
    pub fn schema(topic: EpisodeTopicRequest, revision: TopicRevision) -> Self {
        Self {
            topic,
            revision,
            window: None,
        }
    }
    pub fn with_window(mut self, window: MessageWindow) -> Self {
        self.window = Some(window);
        self
    }
    pub fn topic(&self) -> &EpisodeTopicRequest {
        &self.topic
    }
    pub fn revision(&self) -> &TopicRevision {
        &self.revision
    }
    pub fn window(&self) -> Option<&MessageWindow> {
        self.window.as_ref()
    }
    pub fn episode(&self) -> EpisodeSelection {
        EpisodeSelection::new(
            self.topic.dataset().clone(),
            self.topic.episode_id().to_owned(),
        )
    }
    /// The native reader of this topic revision, as a FROM source.
    pub fn source_sql(&self) -> String {
        format!(
            "spql.mcap.read_topic({}, {}, {})",
            SqlLiteral::text(&self.topic.dataset().sql_name()),
            SqlLiteral::text(self.topic.topic()),
            SqlLiteral::text(self.revision.variant())
        )
    }
    /// The episode query for this read. A schema read returns no rows; a
    /// window reads one row past its limit so a full page is known to continue.
    pub fn sql(&self) -> Result<String, String> {
        let source = self.source_sql();
        match &self.window {
            None => Ok(format!(
                "SELECT {MESSAGE_POSITION}, value FROM {source} WHERE id = $1 LIMIT 0"
            )),
            Some(window) => Ok(format!(
                "SELECT {} FROM {source} WHERE id = $1 {}",
                window.projection()?,
                window.clause()?
            )),
        }
    }
    /// The most rows [`Self::sql`] can return.
    pub fn max_rows(&self) -> usize {
        self.window.as_ref().map_or(1, |window| window.limit + 1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageCursor {
    time: String,
    chunk: String,
    ordinal: u32,
}
impl MessageCursor {
    pub fn new(time: u64, chunk: u64, ordinal: u32) -> Self {
        Self {
            time: time.to_string(),
            chunk: chunk.to_string(),
            ordinal,
        }
    }
    pub fn time(&self) -> Result<u64, String> {
        self.time
            .parse()
            .map_err(|_| "Invalid message cursor timestamp".into())
    }
    pub fn chunk(&self) -> Result<u64, String> {
        self.chunk
            .parse()
            .map_err(|_| "Invalid message cursor chunk offset".into())
    }
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageWindow {
    start: String,
    end: String,
    fields: Vec<Vec<String>>,
    after: Option<MessageCursor>,
    limit: usize,
}
impl MessageWindow {
    pub fn new(bounds: (u64, u64), fields: Vec<Vec<String>>) -> Self {
        Self {
            start: bounds.0.to_string(),
            end: bounds.1.to_string(),
            fields,
            after: None,
            limit: 2048,
        }
    }
    pub fn with_cursor(mut self, cursor: Option<MessageCursor>) -> Self {
        self.after = cursor;
        self
    }
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
    pub fn fields(&self) -> &[Vec<String>] {
        &self.fields
    }
    pub fn after(&self) -> Option<&MessageCursor> {
        self.after.as_ref()
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    fn projection(&self) -> Result<String, String> {
        let mut fields = vec![MESSAGE_POSITION.to_owned()];
        if self.fields.is_empty() {
            fields.push("value".into());
        }
        for (index, path) in self.fields.iter().enumerate() {
            if path.first().map(String::as_str) != Some("value") || path.len() > 16 {
                return Err("Fields must be paths in the message value".into());
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
    fn clause(&self) -> Result<String, String> {
        let (start, end) = self.bounds()?;
        let after = match &self.after {
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
            self.limit + 1
        ))
    }
    pub fn bounds(&self) -> Result<(u64, u64), String> {
        let start = self
            .start
            .parse::<u64>()
            .map_err(|_| "Invalid window start")?;
        let end = self.end.parse::<u64>().map_err(|_| "Invalid window end")?;
        if start >= end || self.limit == 0 || self.limit > 4096 || self.fields.len() > 64 {
            return Err(
                "Message reads require an ordered time window, 1–4096 rows and at most 64 fields"
                    .into(),
            );
        }
        Ok((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::DatasetSelection;

    fn read() -> MessageRead {
        MessageRead::schema(
            EpisodeTopicRequest::new(
                DatasetSelection::new(vec!["dataset_test".into()]).unwrap(),
                "/sensor's".into(),
                "episode".into(),
            ),
            TopicRevision::new("variant".into()),
        )
    }

    #[test]
    fn projections_quote_paths_and_leave_schema_validation_to_spiral() {
        let schema = read().sql().unwrap();
        assert!(schema.contains("read_topic('dataset_test', '/sensor''s', 'variant')"));
        assert!(schema.ends_with("WHERE id = $1 LIMIT 0"));
        let window = MessageWindow::new((1, 10), vec![vec!["value".into(), "a.b\"c".into()]])
            .with_limit(2)
            .with_cursor(Some(MessageCursor::new(5, 7, 3)));
        let windowed = read().with_window(window);
        let sql = windowed.sql().unwrap();
        assert!(sql.contains("\"value\".\"a.b\"\"c\" AS field_0 FROM"));
        assert!(sql.contains("log_time >= 1 AND log_time < 10"));
        assert!(sql.contains("(chunk_start_offset = 7 AND sample_ordinal > 3)"));
        assert!(sql.ends_with("LIMIT 3"));
        assert_eq!(windowed.max_rows(), 3);
        let invalid = read().with_window(MessageWindow::new(
            (1, 10),
            vec![vec!["other".into(), "field".into()]],
        ));
        assert!(invalid.sql().is_err());
        assert!(MessageWindow::new((10, 1), vec![]).bounds().is_err());
    }
}
