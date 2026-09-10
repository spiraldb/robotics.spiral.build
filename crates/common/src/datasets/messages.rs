//! Schema-neutral reads of one immutable native topic revision.
use super::EpisodeTopicRequest;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicRevision {
    version: String,
    variant: String,
}
impl TopicRevision {
    pub fn new(version: String, variant: String) -> Self {
        Self { version, variant }
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn variant(&self) -> &str {
        &self.variant
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
