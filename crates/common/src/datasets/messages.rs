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
            self.topic.episode_id().clone(),
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

/// The value schemas of several topic revisions of one episode, read by one
/// episode query instead of one each. Every revision contributes an empty
/// relation, so the statement returns no rows, only a `value_<index>` column
/// per revision in the order given.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueSchemaRead {
    episode: EpisodeSelection,
    sources: Vec<String>,
}
impl ValueSchemaRead {
    /// At least one read, all of the same episode.
    pub fn new(reads: &[MessageRead]) -> Result<Self, String> {
        let episode = reads
            .first()
            .ok_or("A schema read needs at least one topic")?
            .episode();
        if reads.iter().any(|read| read.episode() != episode) {
            return Err("A schema read covers exactly one episode".into());
        }
        Ok(Self {
            episode,
            sources: reads.iter().map(MessageRead::source_sql).collect(),
        })
    }
    pub fn episode(&self) -> &EpisodeSelection {
        &self.episode
    }
    /// The column holding the value of the read at `index`.
    pub fn column(index: usize) -> String {
        format!("value_{index}")
    }
    pub fn sql(&self) -> String {
        let relations = self
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                format!(
                    "(SELECT value AS {} FROM {source} WHERE id = $1 LIMIT 0) read_{index}",
                    Self::column(index)
                )
            })
            .collect::<Vec<_>>();
        format!("SELECT * FROM {}", relations.join(" CROSS JOIN "))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageCursor {
    time: u64,
    chunk: u64,
    ordinal: u32,
}
impl MessageCursor {
    pub const fn new(time: u64, chunk: u64, ordinal: u32) -> Self {
        Self {
            time,
            chunk,
            ordinal,
        }
    }
    pub const fn time(&self) -> u64 {
        self.time
    }
    pub const fn chunk(&self) -> u64 {
        self.chunk
    }
    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageWindow {
    start: u64,
    end: u64,
    fields: Vec<Vec<String>>,
    after: Option<MessageCursor>,
    limit: usize,
}
impl MessageWindow {
    pub fn new(bounds: (u64, u64), fields: Vec<Vec<String>>) -> Self {
        Self {
            start: bounds.0,
            end: bounds.1,
            fields,
            after: None,
            limit: 2048,
        }
    }
    pub fn with_cursor(mut self, cursor: Option<MessageCursor>) -> Self {
        self.after = cursor;
        self
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
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
                let time = cursor.time();
                let chunk = cursor.chunk();
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
        let (start, end) = (self.start, self.end);
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
    use crate::datasets::{DatasetSelection, EpisodeId};

    fn read() -> MessageRead {
        MessageRead::schema(
            EpisodeTopicRequest::new(
                DatasetSelection::new(vec!["dataset_test".into()]).unwrap(),
                "/sensor's".into(),
                EpisodeId::try_from("01a0989fb65870a7a5459902f9e3e090").unwrap(),
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

    #[test]
    fn one_schema_read_names_each_topic_value_by_its_position() {
        let second = MessageRead::schema(
            EpisodeTopicRequest::new(
                read().topic().dataset().clone(),
                "/other".into(),
                read().topic().episode_id().clone(),
            ),
            TopicRevision::new("next".into()),
        );
        let sql = ValueSchemaRead::new(&[read(), second]).unwrap().sql();
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT value AS value_0 FROM spql.mcap.read_topic('dataset_test', '/sensor''s', 'variant') WHERE id = $1 LIMIT 0) read_0 \
             CROSS JOIN (SELECT value AS value_1 FROM spql.mcap.read_topic('dataset_test', '/other', 'next') WHERE id = $1 LIMIT 0) read_1"
        );
        assert!(ValueSchemaRead::new(&[]).is_err());
        let elsewhere = MessageRead::schema(
            EpisodeTopicRequest::new(
                read().topic().dataset().clone(),
                "/sensor".into(),
                EpisodeId::try_from("01a0989fc80b75e7a89de99ad2bd1d49").unwrap(),
            ),
            TopicRevision::new("variant".into()),
        );
        assert!(ValueSchemaRead::new(&[read(), elsewhere]).is_err());
    }
}
