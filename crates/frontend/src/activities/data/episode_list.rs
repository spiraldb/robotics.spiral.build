//! Episodes of one dataset: identity, source file, timing, and paging.
//!
//! Each native member is one MCAP file. The rail lists them in recording
//! order with the part of the source path that differs between files, so
//! an operator recognizes their own recordings instead of reading UUIDs.
use super::episode_transport::EpisodeTimeRange;
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, UtcTimestampPresentation,
};
use serde::Deserialize;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::SpiralQuerySnapshot;

pub(super) const EPISODE_PAGE_SIZE: usize = 50;

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
    pub(super) fn selection(&self, dataset: &DatasetSelection) -> EpisodeSelection {
        EpisodeSelection::new(dataset.clone(), self.id.clone())
    }
    pub(super) fn time_range(&self) -> Option<EpisodeTimeRange> {
        Some(EpisodeTimeRange::new(
            self.timing.message_start_time?,
            self.timing.message_end_time?,
        ))
    }
    pub(super) fn duration_seconds(&self) -> Option<f64> {
        self.timing.duration_ns.map(|nanos| nanos as f64 / 1e9)
    }
    pub(super) fn duration_label(&self) -> String {
        self.duration_seconds()
            .map(ClockPresentation::clock_label)
            .unwrap_or_else(|| "—".into())
    }
    fn recorded_label(&self) -> String {
        self.timing
            .message_start_time
            .map(UtcTimestampPresentation::utc_label)
            .unwrap_or_else(|| "Unknown time".into())
    }
    /// Everything known about the file, for tooltips: when, how much, how big.
    pub(super) fn facts_label(&self) -> String {
        let mut facts = vec![format!("Recorded {}", self.recorded_label())];
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
        self.timing
            .message_start_time
            .map(UtcTimestampPresentation::utc_date_label)
            .unwrap_or_else(|| "—".into())
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
        SourceLabel::split(relative)
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
}

/// The episode the workspace is inspecting, with the label the rail showed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenedEpisode {
    dataset: DatasetSelection,
    listed: ListedEpisode,
    label: SourceLabel,
}

impl OpenedEpisode {
    pub(super) fn from_comparison(
        dataset: DatasetSelection,
        mut row: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        let id = row
            .remove("episode_id")
            .ok_or("Comparison evidence has no episode identity.")?;
        row.insert("id".into(), id);
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
    pub(super) fn selection(&self) -> EpisodeSelection {
        self.listed.record.selection(&self.dataset)
    }
}

/// One chronological page of a dataset's members.
pub(super) struct EpisodePage {
    episodes: Vec<ListedEpisode>,
    has_more: bool,
}

impl EpisodePage {
    pub(super) fn sql(dataset: &DatasetSelection, offset: usize) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, message_end_time, duration_ns, channel_count \
             FROM spql.mcap.episodes('{}') ORDER BY message_start_time, source_uri LIMIT {} OFFSET {offset}",
            dataset.sql_name().replace('\'', "''"),
            EPISODE_PAGE_SIZE + 1
        )
    }
    pub(super) fn from_snapshot(
        snapshot: SpiralQuerySnapshot,
        offset: usize,
    ) -> Result<Self, String> {
        let has_more = snapshot.rows().len() > EPISODE_PAGE_SIZE || snapshot.truncated();
        let episodes = snapshot
            .into_rows()
            .into_iter()
            .take(EPISODE_PAGE_SIZE)
            .enumerate()
            .map(|(index, row)| {
                serde_json::from_value(serde_json::Value::Object(row))
                    .map(|record| ListedEpisode {
                        ordinal: offset + index + 1,
                        record,
                    })
                    .map_err(|error| format!("Reading episode metadata: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { episodes, has_more })
    }
    pub(super) fn episodes(&self) -> &[ListedEpisode] {
        &self.episodes
    }
    pub(super) fn has_more(&self) -> bool {
        self.has_more
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

    fn row(id: &str, uri: &str, start: u64) -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "id": id, "source_uri": uri, "source_bytes": 15_932_137_u64, "message_count": 11_404_u64,
            "message_start_time": start, "message_end_time": start + 20_501_879_296, "duration_ns": 20_501_879_296_u64,
            "channel_count": 23
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn labels_start_where_source_paths_differ() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![
                    row(
                        "a",
                        "hf://d/x@1/data/val/fold_skirts/episode_aa/episode.fo.mcap",
                        1_743_701_335_734_982_912,
                    ),
                    row(
                        "b",
                        "hf://d/x@1/data/val/place_fruit/episode_bb/episode.fo.mcap",
                        1_744_170_956_524_465_408,
                    ),
                ],
                false,
            ),
            50,
        )
        .unwrap();
        let prefix = page.episodes().shared_source_prefix();
        let first = page.episodes()[0].record().source_label(prefix);
        assert_eq!(first.head(), "fold_skirts");
        assert_eq!(first.tail(), Some("episode_aa/episode.fo.mcap"));
        assert_eq!(page.episodes()[0].ordinal(), 51);
        assert_eq!(
            page.episodes()[0].record().facts_label(),
            "Recorded 2025-04-03 17:28:55 UTC · 11,404 messages · 23 channels · 15.2 MiB"
        );
        assert_eq!(page.episodes()[0].record().duration_label(), "0:20.5");
        assert_eq!(page.episodes()[0].record().message_count_label(), "11,404");
        assert!(!page.has_more());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_single_file_is_labelled_by_its_name_and_bad_rows_are_errors() {
        let page = EpisodePage::from_snapshot(
            SpiralQuerySnapshot::new(
                vec![],
                vec![row("a", "spfsu://project/attempts/take-24.mcap", 0)],
                false,
            ),
            0,
        )
        .unwrap();
        let prefix = page.episodes().shared_source_prefix();
        assert_eq!(
            page.episodes()[0].record().source_label(prefix).head(),
            "take-24.mcap"
        );
        assert!(
            EpisodePage::from_snapshot(
                SpiralQuerySnapshot::new(vec![], vec![Default::default()], false),
                0
            )
            .is_err()
        );
        let dataset = DatasetSelection::new(vec!["owner's recordings".into()]).unwrap();
        assert!(EpisodePage::sql(&dataset, 50).contains("'owner''s recordings'"));
        assert!(EpisodePage::sql(&dataset, 50).ends_with("LIMIT 51 OFFSET 50"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn comparison_evidence_preserves_episode_identity_and_timing() {
        let dataset = DatasetSelection::new(vec!["recordings".into()]).unwrap();
        let mut evidence = row("a", "spfsu://project/take-24.mcap", 1_000);
        let id = evidence.remove("id").unwrap();
        evidence.insert("episode_id".into(), id);
        evidence.insert("id".into(), "comparison-row".into());
        let episode = OpenedEpisode::from_comparison(dataset.clone(), evidence).unwrap();
        assert_eq!(episode.selection().dataset(), &dataset);
        assert_eq!(episode.selection().episode_id(), "a");
        assert_eq!(episode.listed().title(), "a");
        assert_eq!(episode.label().head(), "take-24.mcap");
        assert_eq!(episode.record().time_range().unwrap().start_ns(), 1_000);
        assert!(OpenedEpisode::from_comparison(dataset, Default::default()).is_err());
    }
}
