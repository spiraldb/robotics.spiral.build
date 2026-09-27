//! Which recording an episode is, what is known about it, and what every
//! surface calls it.
//!
//! Each native member is one MCAP file, identified by the id Spiral gave it
//! when it joined the dataset. That id is also its name — "Episode …2635e3d1"
//! is the same file in Data's rail and a Curation index, and
//! stays that file however many recordings are imported around it. A list
//! labels each episode by the part of its source path that differs from its
//! neighbours, so an operator recognizes their own recordings at a glance.
use super::transport::EpisodeTimeRange;
use crate::activities::data::DatasetClient;
use crate::format::{
    ByteCountPresentation, ClockPresentation, GroupedCountPresentation, UtcTimestampPresentation,
    WallClockPresentation,
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;

/// The statistics Spiral lists for every episode of a dataset.
#[derive(Clone, Debug, serde::Deserialize, PartialEq)]
pub(in crate::activities) struct EpisodeFacts {
    id: EpisodeId,
    #[serde(default)]
    source_uri: Option<String>,
    #[serde(default)]
    source_bytes: Option<u64>,
    #[serde(default)]
    message_count: Option<u64>,
    #[serde(default)]
    channel_count: Option<u64>,
    #[serde(default)]
    message_start_time: Option<u64>,
    #[serde(default)]
    duration_ns: Option<u64>,
}

impl EpisodeFacts {
    pub(in crate::activities) fn id(&self) -> &EpisodeId {
        &self.id
    }
    pub(in crate::activities) fn source_uri(&self) -> Option<&str> {
        self.source_uri.as_deref()
    }
    pub(in crate::activities) const fn source_bytes(&self) -> Option<u64> {
        self.source_bytes
    }
    pub(in crate::activities) const fn message_count(&self) -> Option<u64> {
        self.message_count
    }
    pub(in crate::activities) const fn channel_count(&self) -> Option<u64> {
        self.channel_count
    }
    pub(in crate::activities) const fn message_start_time(&self) -> Option<u64> {
        self.message_start_time
    }
    pub(in crate::activities) const fn duration_ns(&self) -> Option<u64> {
        self.duration_ns
    }
    pub(in crate::activities) fn duration_seconds(&self) -> Option<f64> {
        self.duration_ns.map(|nanos| nanos as f64 / 1e9)
    }
    /// The file name at the end of the source path, when there is one.
    fn file_name(&self) -> &str {
        self.source_uri
            .as_deref()
            .and_then(|uri| uri.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .unwrap_or(self.id.as_str())
    }
}

/// One episode of a dataset, as every surface opens and names it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct Episode {
    dataset: DatasetSelection,
    facts: EpisodeFacts,
}

/// One page of a dataset's episodes, and whether more follow it.
pub(in crate::activities) struct EpisodePage {
    episodes: Vec<Episode>,
    has_more: bool,
}

impl EpisodePage {
    pub(in crate::activities) fn into_episodes(self) -> Vec<Episode> {
        self.episodes
    }
    pub(in crate::activities) fn episodes(&self) -> &[Episode] {
        &self.episodes
    }
    pub(in crate::activities) fn has_more(&self) -> bool {
        self.has_more
    }
}

impl Episode {
    /// An episode whose facts a caller already read.
    pub(in crate::activities) fn new(dataset: DatasetSelection, facts: EpisodeFacts) -> Self {
        Self { dataset, facts }
    }

    /// Up to `limit` of a dataset's episodes from `offset`, in recording order.
    pub(in crate::activities) async fn page(
        dataset: &DatasetSelection,
        offset: usize,
        limit: usize,
    ) -> Result<EpisodePage, String> {
        let snapshot = DatasetClient::current()
            .run_spiral_query(Self::page_sql(dataset, offset, limit), limit + 1)
            .await
            .map_err(|error| error.to_string())?;
        let has_more = snapshot.rows().len() > limit || snapshot.truncated();
        let episodes = snapshot
            .into_rows()
            .into_iter()
            .take(limit)
            .map(|row| Self::from_row(dataset, row))
            .collect::<Result<_, _>>()?;
        Ok(EpisodePage { episodes, has_more })
    }

    /// How many episodes `dataset` holds.
    pub(in crate::activities) async fn count(dataset: &DatasetSelection) -> Result<u64, String> {
        let sql = format!(
            "SELECT count(*) AS episodes FROM spql.mcap.episodes({})",
            SqlLiteral::text(&dataset.sql_name())
        );
        DatasetClient::current()
            .run_spiral_query(sql, 1)
            .await
            .map_err(|error| error.to_string())?
            .into_rows()
            .into_iter()
            .next()
            .and_then(|row| row.get("episodes").and_then(serde_json::Value::as_u64))
            .ok_or_else(|| "Counting episodes returned no row".to_owned())
    }

    /// One episode by its id.
    pub(in crate::activities) async fn open(selection: EpisodeSelection) -> Result<Self, String> {
        let dataset = selection.dataset().clone();
        let sql = format!(
            "SELECT * FROM ({}) listed WHERE id = $1",
            Self::listing_sql(&dataset)
        );
        let row = DatasetClient::current()
            .run_episode_query(&EpisodeQueryRequest::new(selection, sql, 1))
            .await
            .map_err(|error| error.to_string())?
            .into_rows()
            .into_iter()
            .next()
            .ok_or("This episode is no longer in the dataset.")?;
        Self::from_row(&dataset, row)
    }

    /// Every episode of `dataset` with the facts every surface shows.
    pub(in crate::activities) fn listing_sql(dataset: &DatasetSelection) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, duration_ns, channel_count FROM spql.mcap.episodes({})",
            SqlLiteral::text(&dataset.sql_name())
        )
    }

    /// The first `count` episodes of `dataset`, in recording order: the ones
    /// every page and sample starts from.
    pub(in crate::activities) fn first_sql(dataset: &DatasetSelection, count: usize) -> String {
        format!(
            "{} ORDER BY message_start_time, source_uri, id LIMIT {count}",
            Self::listing_sql(dataset)
        )
    }

    /// In recording order, with one more row than the page holds, which says
    /// whether another follows.
    fn page_sql(dataset: &DatasetSelection, offset: usize, limit: usize) -> String {
        format!("{} OFFSET {offset}", Self::first_sql(dataset, limit + 1))
    }

    fn from_row(
        dataset: &DatasetSelection,
        row: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, String> {
        serde_json::from_value(serde_json::Value::Object(row))
            .map(|facts| Self::new(dataset.clone(), facts))
            .map_err(|error| format!("Reading episode metadata: {error}"))
    }

    pub(in crate::activities) fn id(&self) -> &EpisodeId {
        self.facts.id()
    }
    pub(in crate::activities) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(in crate::activities) fn facts(&self) -> &EpisodeFacts {
        &self.facts
    }
    pub(in crate::activities) fn selection(&self) -> EpisodeSelection {
        EpisodeSelection::new(self.dataset.clone(), self.facts.id().clone())
    }
    /// What every heading calls this episode.
    pub(in crate::activities) fn name(&self) -> String {
        Self::name_of(self.id())
    }
    /// What every heading calls the episode with `id`, for a surface that
    /// holds the id before it has the episode.
    pub(in crate::activities) fn name_of(id: &EpisodeId) -> String {
        format!("Episode …{}", id.short())
    }
    /// What a list of episodes calls one: its id alone, since the list
    /// already says what its rows are.
    pub(in crate::activities) fn listed_name_of(id: &EpisodeId) -> String {
        format!("…{}", id.short())
    }
    /// The recorded span, from the first message to the last.
    pub(in crate::activities) fn range(&self) -> Option<EpisodeTimeRange> {
        let start = self.facts.message_start_time()?;
        let end = start.checked_add(self.facts.duration_ns()?)?;
        Some(EpisodeTimeRange::new(start, end))
    }
    /// Whether a search term appears in its id or source path, whatever its
    /// case — the one test every list of episodes filters by.
    pub(in crate::activities) fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        needle.is_empty()
            || self.id().as_str().contains(&needle)
            || self
                .facts
                .source_uri()
                .is_some_and(|uri| uri.to_lowercase().contains(&needle))
    }

    /// One episode of a fixed length, for tests that only need its identity
    /// and duration.
    #[cfg(test)]
    pub(in crate::activities) fn fixture(id: &EpisodeId, duration_ns: u64) -> Self {
        let facts = serde_json::from_value(serde_json::json!({
            "id": id,
            "source_uri": format!("s3://recordings/{id}.mcap"),
            "message_start_time": 1_000_000_000_u64,
            "duration_ns": duration_ns,
        }))
        .expect("a fixture episode");
        Self::new(
            DatasetSelection::new(vec!["folding".into()]).expect("a dataset name"),
            facts,
        )
    }
}

/// What an operator reads about an episode, worded the same on every surface.
pub(in crate::activities) trait EpisodeFactsPresentation {
    fn duration_label(&self) -> String;
    fn message_count_label(&self) -> String;
    /// The day it was recorded, or why that is unknown.
    fn recorded_date_label(&self) -> String;
    /// Everything known about the file on one line, for tooltips.
    fn facts_label(&self) -> String;
}

impl EpisodeFactsPresentation for EpisodeFacts {
    fn duration_label(&self) -> String {
        self.duration_seconds()
            .map_or_else(|| "—".into(), ClockPresentation::clock_label)
    }
    fn message_count_label(&self) -> String {
        self.message_count()
            .map_or_else(|| "—".into(), GroupedCountPresentation::grouped_label)
    }
    fn recorded_date_label(&self) -> String {
        match self.message_start_time() {
            Some(start) if start.has_wall_clock() => start.utc_date_label(),
            Some(_) => "no wall clock".into(),
            None => "—".into(),
        }
    }
    fn facts_label(&self) -> String {
        let mut facts = vec![match self.message_start_time() {
            Some(start) if start.has_wall_clock() => format!("Recorded {}", start.utc_label()),
            Some(_) => "Recorded without a wall clock".into(),
            None => "Recorded at an unknown time".into(),
        }];
        facts.push(format!("{} messages", self.message_count_label()));
        if let Some(channels) = self.channel_count() {
            facts.push(format!("{channels} channels"));
        }
        if let Some(bytes) = self.source_bytes() {
            facts.push(bytes.byte_label());
        }
        facts.join(" · ")
    }
}

/// Where the source paths of a list of episodes start to differ.
pub(in crate::activities) struct EpisodeLabels {
    shared_prefix: usize,
}

impl EpisodeLabels {
    pub(in crate::activities) fn of<'a>(
        episodes: impl IntoIterator<Item = &'a EpisodeFacts>,
    ) -> Self {
        let mut uris = episodes.into_iter().filter_map(EpisodeFacts::source_uri);
        let Some(mut prefix) = uris.next() else {
            return Self { shared_prefix: 0 };
        };
        for uri in uris {
            let common = prefix
                .char_indices()
                .zip(uri.chars())
                .take_while(|((_, left), right)| left == right)
                .last()
                .map_or(0, |((index, character), _)| index + character.len_utf8());
            prefix = &prefix[..common];
        }
        // Cut back to a directory boundary so a label never starts mid-word.
        Self {
            shared_prefix: prefix.rfind('/').map_or(0, |slash| slash + 1),
        }
    }
    /// The source path below the directory every listed episode shares.
    pub(in crate::activities) fn label(&self, facts: &EpisodeFacts) -> SourceLabel {
        let relative = facts
            .source_uri()
            .and_then(|uri| uri.get(self.shared_prefix..))
            .filter(|relative| !relative.is_empty())
            .unwrap_or_else(|| facts.file_name());
        SourceLabel::of(relative, facts)
    }
}

/// The distinguishing directory, in words. A recording whose path says
/// nothing an operator would recognize has no label at all, and is known by
/// its id alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::activities) struct SourceLabel {
    head: Option<String>,
}

impl SourceLabel {
    /// An identifier-shaped head tells an operator nothing, so the recording
    /// time takes its place when there is one. The full source path stays in
    /// the tooltip.
    fn of(relative: &str, facts: &EpisodeFacts) -> Self {
        let head = match relative.split_once('/') {
            Some((head, tail)) if !head.is_empty() && !tail.is_empty() => head,
            _ => relative,
        };
        let head = if Self::is_identifier(head) {
            facts
                .message_start_time()
                .filter(|start| start.has_wall_clock())
                .map(|start| format!("Recorded {}", start.utc_time_label()))
        } else {
            Some(head.replace('_', " "))
        };
        Self { head }
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
    pub(in crate::activities) fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset() -> DatasetSelection {
        DatasetSelection::new(vec!["recordings".into()]).unwrap()
    }

    /// Listed episodes whose ids are the fixtures `1, 2, …` in list order.
    fn episodes(rows: &[(&str, u64)]) -> Vec<Episode> {
        rows.iter()
            .zip(1..)
            .map(|((uri, start), ordinal)| {
                let row = serde_json::json!({
                    "id": EpisodeId::fixture(ordinal), "source_uri": uri,
                    "source_bytes": 15_932_137_u64, "message_count": 11_404_u64,
                    "message_start_time": start, "duration_ns": 20_501_879_296_u64,
                    "channel_count": 23,
                });
                Episode::from_row(&dataset(), row.as_object().unwrap().clone()).unwrap()
            })
            .collect()
    }

    fn labels(episodes: &[Episode]) -> Vec<SourceLabel> {
        let labels = EpisodeLabels::of(episodes.iter().map(Episode::facts));
        episodes
            .iter()
            .map(|episode| labels.label(episode.facts()))
            .collect()
    }

    #[test]
    fn an_episode_is_named_by_its_id_alone() {
        let id = EpisodeId::try_from("01a0989fb6557788a2dd36342635e3d1").unwrap();
        let episode = Episode::fixture(&id, 1);
        assert_eq!(episode.name(), "Episode …2635e3d1");
        assert_eq!(Episode::name_of(&id), episode.name());
        assert_eq!(episode.selection().episode_id(), &id);
    }

    #[test]
    fn labels_start_where_source_paths_differ_and_read_as_words() {
        let listed = episodes(&[
            (
                "hf://d/x@1/data/val/fold_skirts/episode_aa/episode.fo.mcap",
                1_743_701_335_734_982_912,
            ),
            (
                "hf://d/x@1/data/val/place_fruit/episode_bb/episode.fo.mcap",
                1_744_170_956_524_465_408,
            ),
        ]);
        let [first, second] = labels(&listed).try_into().unwrap();
        assert_eq!(first.head(), Some("fold skirts"));
        assert_eq!(second.head(), Some("place fruit"));
        let episode = &listed[0];
        assert_eq!(
            episode.facts().facts_label(),
            "Recorded 2025-04-03 17:28:55 UTC · 11,404 messages · 23 channels · 15.2 MiB"
        );
        assert_eq!(episode.facts().duration_label(), "0:20.5");
        assert_eq!(episode.facts().message_count_label(), "11,404");
        assert_eq!(
            episode.range().map(EpisodeTimeRange::bounds),
            Some((1_743_701_335_734_982_912, 1_743_701_356_236_862_209))
        );
    }

    #[test]
    fn identifier_named_files_read_as_recording_times_or_go_by_their_id() {
        let listed = episodes(&[
            (
                "spfsu://p/attempts/01a07ed2-1ed5-7542-bbf3-15a78c389242/blobs/01a07ed2-1fdd-77a2-a9ae-5c3bbca09151.bin",
                1_743_701_335_734_982_912,
            ),
            (
                "spfsu://p/attempts/01a0b638-0359-74d2-b28c-3daf15a6ecfa/blobs/01a0b638-099b-7740-a44b-ef08485546ed.bin",
                0,
            ),
        ]);
        let [clocked, epoch] = labels(&listed).try_into().unwrap();
        assert_eq!(clocked.head(), Some("Recorded 17:28:55 UTC"));
        assert_eq!(listed[0].facts().recorded_date_label(), "2025-04-03");
        assert_eq!(epoch.head(), None);
        assert_eq!(listed[1].facts().recorded_date_label(), "no wall clock");
        assert!(
            listed[1]
                .facts()
                .facts_label()
                .starts_with("Recorded without a wall clock · ")
        );
    }

    #[test]
    fn a_single_file_is_labelled_by_its_name() {
        let listed = episodes(&[("file:///runs/take-7.mcap", 0)]);
        assert_eq!(labels(&listed)[0].head(), Some("take-7.mcap"));
        assert!(!SourceLabel::is_identifier("fold_and_stack_the_skirts"));
        assert!(SourceLabel::is_identifier(
            "01a0b638-099b-7740-a44b-ef08485546ed.bin"
        ));
    }

    #[test]
    fn a_search_term_matches_the_source_path_or_id_regardless_of_case() {
        let listed = episodes(&[
            ("hf://d/x@1/data/Fold_Skirts/episode_aa/episode.fo.mcap", 1),
            ("hf://d/x@1/data/place_fruit/episode_bb/episode.fo.mcap", 2),
        ]);
        let [skirts, fruit] = listed.as_slice() else {
            panic!("two episodes");
        };
        assert!(skirts.matches("fold"));
        assert!(!fruit.matches("fold"));
        assert!(fruit.matches(fruit.id().short()));
        assert!(!skirts.matches(fruit.id().short()));
        assert!(fruit.matches(""));
    }

    #[test]
    fn a_page_reads_in_recording_order_and_bad_rows_are_errors() {
        assert!(Episode::from_row(&dataset(), Default::default()).is_err());
        let named = serde_json::json!({ "id": "Episode 7" });
        assert!(Episode::from_row(&dataset(), named.as_object().unwrap().clone()).is_err());
        let owned = DatasetSelection::new(vec!["owner's recordings".into()]).unwrap();
        let sql = Episode::page_sql(&owned, 50, 50);
        assert!(sql.contains("spql.mcap.episodes('owner''s recordings')"));
        assert!(sql.ends_with("ORDER BY message_start_time, source_uri, id LIMIT 51 OFFSET 50"));
    }
}
