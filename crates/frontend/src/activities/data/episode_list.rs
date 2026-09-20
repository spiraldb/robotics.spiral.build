//! Episodes of one dataset: identity, source file, timing, and paging.
//!
//! Each native member is one MCAP file. The rail lists them in recording
//! order with the part of the source path that differs between files, so
//! an operator recognizes their own recordings instead of reading UUIDs.
use super::episode_transport::EpisodeTimeRange;
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, UtcTimestampPresentation,
    WallClockPresentation,
};
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct EpisodeRecord {
    id: String,
    #[serde(flatten)]
    source: EpisodeSource,
    #[serde(flatten)]
    timing: EpisodeTiming,
    message_count: Option<u64>,
    channel_count: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeSource {
    source_uri: Option<String>,
    source_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeTiming {
    message_start_time: Option<u64>,
    message_end_time: Option<u64>,
    duration_ns: Option<u64>,
}

impl EpisodeRecord {
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    fn selection(&self, dataset: &DatasetSelection) -> EpisodeSelection {
        EpisodeSelection::new(dataset.clone(), self.id.clone())
    }
    pub(super) fn time_range(&self) -> Option<EpisodeTimeRange> {
        Some(EpisodeTimeRange::new(
            self.timing.message_start_time?,
            self.timing.message_end_time?,
        ))
    }
    fn duration_seconds(&self) -> Option<f64> {
        self.timing.duration_ns.map(|nanos| nanos as f64 / 1e9)
    }
    pub(super) fn duration_label(&self) -> String {
        self.duration_seconds()
            .map(ClockPresentation::clock_label)
            .unwrap_or_else(|| "—".into())
    }
    fn recorded_label(&self) -> String {
        match self.timing.message_start_time {
            Some(start) if start.has_wall_clock() => format!("Recorded {}", start.utc_label()),
            Some(_) => "Recorded without a wall clock".into(),
            None => "Recorded at an unknown time".into(),
        }
    }
    /// What leads an episode row when its file name is only an identifier.
    fn recorded_head(&self) -> String {
        match self.timing.message_start_time {
            Some(start) if start.has_wall_clock() => {
                format!("Recorded {}", start.utc_time_label())
            }
            _ => "Unnamed recording".into(),
        }
    }
    /// Everything known about the file, for tooltips: when, how much, how big.
    pub(super) fn facts_label(&self) -> String {
        let mut facts = vec![self.recorded_label()];
        facts.push(format!("{} messages", self.message_count_label()));
        if let Some(channels) = self.channel_count {
            facts.push(format!("{channels} channels"));
        }
        if let Some(bytes) = self.source_bytes_label() {
            facts.push(bytes);
        }
        facts.join(" · ")
    }
    pub(super) fn recorded_date_label(&self) -> String {
        match self.timing.message_start_time {
            Some(start) if start.has_wall_clock() => start.utc_date_label(),
            Some(_) => "no wall clock".into(),
            None => "—".into(),
        }
    }
    pub(super) fn message_count_label(&self) -> String {
        self.message_count
            .map(GroupedCountPresentation::grouped_label)
            .unwrap_or_else(|| "—".into())
    }
    pub(super) fn source_uri(&self) -> Option<&str> {
        self.source.source_uri.as_deref()
    }
    fn source_bytes_label(&self) -> Option<String> {
        self.source
            .source_bytes
            .map(ByteCountPresentation::byte_label)
    }
    fn file_name(&self) -> &str {
        self.source_uri()
            .and_then(|uri| uri.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }
    /// The source path below the directory every listed episode shares.
    pub(super) fn source_label(&self, shared_prefix: usize) -> SourceLabel {
        let relative = self
            .source_uri()
            .and_then(|uri| uri.get(shared_prefix..))
            .filter(|relative| !relative.is_empty())
            .unwrap_or_else(|| self.file_name());
        SourceLabel::split(relative).named_for(self)
    }
}

/// The distinguishing directory leads; the rest of the path follows quietly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceLabel {
    head: String,
    tail: Option<String>,
}

impl SourceLabel {
    fn split(relative: &str) -> Self {
        match relative.split_once('/') {
            Some((head, tail)) if !head.is_empty() && !tail.is_empty() => Self {
                head: head.to_owned(),
                tail: Some(tail.to_owned()),
            },
            _ => Self {
                head: relative.to_owned(),
                tail: None,
            },
        }
    }
    /// An identifier-shaped head tells an operator nothing, so the recording
    /// time takes its place, and an identifier-shaped tail steps aside. The
    /// full source path stays in the tooltip.
    fn named_for(mut self, record: &EpisodeRecord) -> Self {
        if Self::is_identifier(&self.head) {
            self.head = record.recorded_head();
        }
        if self
            .tail
            .as_deref()
            .is_some_and(|tail| tail.rsplit('/').next().is_some_and(Self::is_identifier))
        {
            self.tail = None;
        }
        self
    }
    /// Hex digits joined by dashes or underscores, long enough to be a UUID
    /// or a content hash; a file extension does not change the verdict.
    fn is_identifier(text: &str) -> bool {
        let stem = text.rsplit_once('.').map_or(text, |(stem, _)| stem);
        let digits = stem
            .chars()
            .filter(|character| !matches!(character, '-' | '_'))
            .collect::<Vec<_>>();
        digits.len() >= 24 && digits.iter().all(char::is_ascii_hexdigit)
    }
    pub(super) fn head(&self) -> &str {
        &self.head
    }
    pub(super) fn tail(&self) -> Option<&str> {
        self.tail.as_deref()
    }
}

/// One episode at its chronological position within the dataset.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ListedEpisode {
    ordinal: usize,
    record: EpisodeRecord,
}

impl ListedEpisode {
    pub(super) fn new(ordinal: usize, record: EpisodeRecord) -> Self {
        Self { ordinal, record }
    }
    pub(super) fn ordinal(&self) -> usize {
        self.ordinal
    }
    pub(super) fn record(&self) -> &EpisodeRecord {
        &self.record
    }
    pub(super) fn title(&self) -> String {
        if self.ordinal == 0 {
            self.record.id().to_owned()
        } else {
            format!("#{}", self.ordinal)
        }
    }
    /// Whether a lowercase search term appears in the ordinal or source path.
    pub(super) fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.title().to_lowercase().contains(needle)
            || self
                .record
                .source_uri()
                .is_some_and(|uri| uri.to_lowercase().contains(needle))
    }
}

/// The episode the workspace is inspecting, with the label the rail showed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OpenedEpisode {
    dataset: DatasetSelection,
    listed: ListedEpisode,
    label: SourceLabel,
}

impl OpenedEpisode {
    pub(in crate::activities) fn from_row(
        dataset: DatasetSelection,
        row: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        let record: EpisodeRecord = serde_json::from_value(serde_json::Value::Object(row))
            .map_err(|error| format!("Reading episode identity: {error}"))?;
        let label = SourceLabel::split(record.file_name());
        Ok(Self::new(
            dataset,
            ListedEpisode { ordinal: 0, record },
            label,
        ))
    }

    pub(super) fn new(
        dataset: DatasetSelection,
        listed: ListedEpisode,
        label: SourceLabel,
    ) -> Self {
        Self {
            dataset,
            listed,
            label,
        }
    }
    pub(super) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(super) fn listed(&self) -> &ListedEpisode {
        &self.listed
    }
    pub(super) fn record(&self) -> &EpisodeRecord {
        &self.listed.record
    }
    pub(super) fn label(&self) -> &SourceLabel {
        &self.label
    }
    pub(in crate::activities) fn title(&self) -> String {
        self.listed.title()
    }
    pub(in crate::activities) fn selection(&self) -> EpisodeSelection {
        self.listed.record.selection(&self.dataset)
    }
}

/// Directory prefix shared by every listed source, so labels start where the
/// files start to differ.
pub(super) trait SharedSourcePrefix {
    fn shared_source_prefix(&self) -> usize;
}

impl SharedSourcePrefix for [ListedEpisode] {
    fn shared_source_prefix(&self) -> usize {
        let mut uris = self
            .iter()
            .filter_map(|episode| episode.record.source_uri());
        let Some(first) = uris.next() else {
            return 0;
        };
        let mut prefix = first;
        for uri in uris {
            let common = prefix
                .char_indices()
                .zip(uri.chars())
                .take_while(|((_, left), right)| left == right)
                .last()
                .map_or(0, |((index, character), _)| index + character.len_utf8());
            prefix = &prefix[..common];
        }
        prefix.rfind('/').map_or(0, |slash| slash + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn source_labels_distinguish_names_from_identifiers() {
        assert!(!SourceLabel::is_identifier("fold_and_stack_the_skirts"));
        assert!(SourceLabel::is_identifier(
            "01a0b638-099b-7740-a44b-ef08485546ed.bin"
        ));
    }
}
