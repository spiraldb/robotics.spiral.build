//! Message density of one topic across an episode's time range.
//!
//! The Data timeline draws one row per recorded stream. A row needs to show
//! *when* the stream has messages without downloading the messages, so the
//! backend counts `log_time` values into a fixed number of equal buckets and
//! returns only the counts.
use super::MessageRead;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicCoverageRequest {
    /// Topic identity plus the window whose bounds define the bucket span.
    read: MessageRead,
    buckets: u16,
}

impl TopicCoverageRequest {
    pub const MIN_BUCKETS: u16 = 8;
    pub const MAX_BUCKETS: u16 = 2048;

    pub fn new(read: MessageRead, buckets: u16) -> Self {
        Self { read, buckets }
    }
    pub fn read(&self) -> &MessageRead {
        &self.read
    }
    pub const fn buckets(&self) -> u16 {
        self.buckets
    }
    /// The half-open nanosecond span the buckets divide.
    pub fn bounds(&self) -> Result<(u64, u64), String> {
        if !(Self::MIN_BUCKETS..=Self::MAX_BUCKETS).contains(&self.buckets) {
            return Err(format!(
                "Coverage needs between {} and {} buckets",
                Self::MIN_BUCKETS,
                Self::MAX_BUCKETS
            ));
        }
        let window = self.read.window().ok_or("Coverage needs a time window")?;
        if !window.fields().is_empty() || window.after().is_some() {
            return Err("Coverage reads log times only; fields and cursors are not allowed".into());
        }
        window.bounds()
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicCoverage {
    messages: u64,
    /// Nanoseconds travel as strings so JavaScript never rounds them.
    first_ns: Option<String>,
    last_ns: Option<String>,
    buckets: Vec<u32>,
}

impl TopicCoverage {
    pub fn empty(buckets: u16) -> Self {
        Self {
            messages: 0,
            first_ns: None,
            last_ns: None,
            buckets: vec![0; usize::from(buckets)],
        }
    }

    /// Count one message. Times outside `bounds` fall into the nearest edge
    /// bucket rather than being dropped, so a count never disagrees with the
    /// message total.
    pub fn observe(&mut self, log_time: u64, bounds: (u64, u64)) {
        let (start, end) = bounds;
        let buckets = self.buckets.len();
        if buckets == 0 {
            return;
        }
        let span = u128::from(end.saturating_sub(start).max(1));
        let offset = u128::from(log_time.saturating_sub(start));
        let index = (offset * buckets as u128 / span).min(buckets as u128 - 1) as usize;
        self.buckets[index] = self.buckets[index].saturating_add(1);
        self.messages = self.messages.saturating_add(1);
        let first = self
            .first_ns
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok());
        if first.is_none_or(|first| log_time < first) {
            self.first_ns = Some(log_time.to_string());
        }
        let last = self
            .last_ns
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok());
        if last.is_none_or(|last| log_time > last) {
            self.last_ns = Some(log_time.to_string());
        }
    }

    pub const fn messages(&self) -> u64 {
        self.messages
    }
    pub fn first_ns(&self) -> Option<u64> {
        self.first_ns
            .as_deref()
            .and_then(|value| value.parse().ok())
    }
    pub fn last_ns(&self) -> Option<u64> {
        self.last_ns.as_deref().and_then(|value| value.parse().ok())
    }
    pub fn buckets(&self) -> &[u32] {
        &self.buckets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::{DatasetSelection, EpisodeTopicRequest, MessageWindow, TopicRevision};

    fn read(window: Option<MessageWindow>) -> MessageRead {
        let read = MessageRead::schema(
            EpisodeTopicRequest::new(
                DatasetSelection::new(vec!["dataset_default".into()]).unwrap(),
                "/joints".into(),
                "01a0989fb65870a7a5459902f9e3e090".into(),
            ),
            TopicRevision::new("v1".into(), "variant".into()),
        );
        match window {
            Some(window) => read.with_window(window),
            None => read,
        }
    }

    #[test]
    fn coverage_requests_need_a_plain_window_and_a_sane_bucket_count() {
        assert!(TopicCoverageRequest::new(read(None), 100).bounds().is_err());
        let fields = MessageWindow::new((0, 10), vec![vec!["value".into()]]);
        assert!(
            TopicCoverageRequest::new(read(Some(fields)), 100)
                .bounds()
                .is_err()
        );
        let plain = MessageWindow::new((0, 10), vec![]);
        assert!(
            TopicCoverageRequest::new(read(Some(plain.clone())), 4)
                .bounds()
                .is_err()
        );
        assert_eq!(
            TopicCoverageRequest::new(read(Some(plain)), 100).bounds(),
            Ok((0, 10))
        );
    }

    #[test]
    fn buckets_cover_the_span_evenly_and_edges_are_kept() {
        let mut coverage = TopicCoverage::empty(4);
        let bounds = (1_000, 1_400);
        for time in [1_000, 1_099, 1_100, 1_399, 1_400, 999] {
            coverage.observe(time, bounds);
        }
        assert_eq!(coverage.buckets(), &[3, 1, 0, 2]);
        assert_eq!(coverage.messages(), 6);
        assert_eq!(coverage.first_ns(), Some(999));
        assert_eq!(coverage.last_ns(), Some(1_400));
        let json = serde_json::to_value(&coverage).unwrap();
        assert_eq!(json["first_ns"], "999");
    }
}
