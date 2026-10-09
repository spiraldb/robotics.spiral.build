//! How a dataset lists its episodes. `spql.mcap.episodes` lists different
//! facts for each producer — an MCAP file's path, size and channels; a
//! LeRobot episode's index and tasks — so a listing is written for the
//! dataset's producer, and a fact the producer does not record is absent
//! rather than an error.
use super::identity::{Episode, EpisodePage};
use crate::activities::data::DatasetClient;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetProducer, DatasetSelection, EpisodeSelection,
};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;

/// One dataset and the producer that made it, which decide how its episodes
/// are listed. A curation keeps it as the dataset it reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct EpisodeListing {
    #[serde(flatten)]
    dataset: DatasetSelection,
    /// A curation saved before Studio read LeRobot datasets names no
    /// producer; its dataset is one of MCAP recordings.
    #[serde(default = "EpisodeListing::recorded_before_producers")]
    producer: DatasetProducer,
}

impl EpisodeListing {
    pub(crate) fn new(dataset: DatasetSelection, producer: DatasetProducer) -> Self {
        Self { dataset, producer }
    }
    /// The listing of the dataset `inspection` read.
    pub(in crate::activities) fn of(inspection: &DatasetInspection) -> Self {
        Self::new(
            inspection.dataset().selection().clone(),
            inspection.producer(),
        )
    }
    /// The listing of `dataset`, asking the catalog which producer made it.
    pub(in crate::activities) async fn read(dataset: &DatasetSelection) -> Result<Self, String> {
        DatasetClient::current()
            .inspect_dataset(dataset.clone())
            .await
            .map(|inspection| Self::of(&inspection))
            .map_err(|error| error.to_string())
    }
    fn recorded_before_producers() -> DatasetProducer {
        DatasetProducer::Mcap
    }
    pub(crate) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(crate) fn producer(&self) -> DatasetProducer {
        self.producer
    }

    /// The facts listed beside each episode's id.
    fn listed_facts(&self) -> &'static [&'static str] {
        match self.producer {
            DatasetProducer::Mcap => &[
                "source_uri",
                "source_bytes",
                "message_count",
                "message_start_time",
                "duration_ns",
                "channel_count",
            ],
            DatasetProducer::LeRobot => &[
                "episode_index",
                "tasks",
                "message_count",
                "message_start_time",
                "duration_ns",
            ],
        }
    }
    /// The order every list of the dataset's episodes follows: when each
    /// recording started, or a LeRobot dataset's own order, whose episodes
    /// each start at zero.
    fn recording_order(&self) -> &'static str {
        match self.producer {
            DatasetProducer::Mcap => "message_start_time, source_uri, id",
            DatasetProducer::LeRobot => "episode_index, id",
        }
    }

    /// Every episode of the dataset with the facts every surface shows.
    pub(in crate::activities) fn sql(&self) -> String {
        let facts = match self.producer {
            DatasetProducer::Mcap => {
                "source.uri AS source_uri, source.size AS source_bytes, message_count, \
                 message_start_time, duration_ns, channel_count"
            }
            DatasetProducer::LeRobot => {
                "episode_index, tasks, message_count, message_start_time, duration_ns"
            }
        };
        format!(
            "SELECT id, {facts} FROM spql.mcap.episodes({})",
            SqlLiteral::text(&self.dataset.sql_name())
        )
    }

    /// The first `count` episodes, in recording order: the ones every page
    /// and sample starts from.
    pub(in crate::activities) fn first_sql(&self, count: usize) -> String {
        format!(
            "{} ORDER BY {} LIMIT {count}",
            self.sql(),
            self.recording_order()
        )
    }

    /// Every row of `rows`, a statement whose rows each name an episode in
    /// `episode_id`, with that episode's listed facts beside it, each named
    /// after `prefix`: one read answers both what the rows hold and which
    /// episodes they belong to, however far into the dataset.
    pub(in crate::activities) fn beside_sql(&self, rows: &str, prefix: &str) -> String {
        let facts = self
            .listed_facts()
            .iter()
            .map(|fact| format!("listed.{fact} AS {prefix}{fact}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "SELECT named.*, {facts} FROM ({rows}) named JOIN ({}) listed ON listed.id = named.episode_id",
            self.sql()
        )
    }

    /// The episode a row read by `beside_sql` names, from the facts under
    /// `prefix`.
    pub(in crate::activities) fn beside(
        &self,
        row: &serde_json::Map<String, serde_json::Value>,
        prefix: &str,
    ) -> Result<Episode, String> {
        let id = row
            .get("episode_id")
            .cloned()
            .ok_or("A row names no episode.")?;
        let facts = self
            .listed_facts()
            .iter()
            .filter_map(|fact| {
                Some((
                    (*fact).to_owned(),
                    row.get(&format!("{prefix}{fact}"))?.clone(),
                ))
            })
            .chain(std::iter::once(("id".to_owned(), id)))
            .collect();
        self.episode(facts)
    }

    /// Up to `limit` episodes from `offset`, in recording order.
    pub(in crate::activities) async fn page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<EpisodePage, String> {
        let snapshot = DatasetClient::current()
            .run_spiral_query(self.page_sql(offset, limit), limit + 1)
            .await
            .map_err(|error| error.to_string())?;
        let has_more = snapshot.rows().len() > limit || snapshot.truncated();
        let episodes = snapshot
            .into_rows()
            .into_iter()
            .take(limit)
            .map(|row| self.episode(row))
            .collect::<Result<_, _>>()?;
        Ok(EpisodePage::new(episodes, has_more))
    }

    /// One episode by its id.
    pub(in crate::activities) async fn open(
        &self,
        selection: EpisodeSelection,
    ) -> Result<Episode, String> {
        let sql = format!("SELECT * FROM ({}) listed WHERE id = $1", self.sql());
        let row = DatasetClient::current()
            .run_episode_query(&EpisodeQueryRequest::new(selection, sql, 1))
            .await
            .map_err(|error| error.to_string())?
            .into_rows()
            .into_iter()
            .next()
            .ok_or("This episode is no longer in the dataset.")?;
        self.episode(row)
    }

    /// In recording order, with one more row than the page holds, which says
    /// whether another follows.
    fn page_sql(&self, offset: usize, limit: usize) -> String {
        format!("{} OFFSET {offset}", self.first_sql(limit + 1))
    }

    /// The episode one row of the listing names.
    pub(super) fn episode(
        &self,
        row: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Episode, String> {
        serde_json::from_value(serde_json::Value::Object(row))
            .map(|facts| Episode::new(self.clone(), facts))
            .map_err(|error| format!("Reading episode metadata: {error}"))
    }

    /// A dataset of MCAP recordings named `name`, for tests.
    #[cfg(test)]
    pub(crate) fn mcap_fixture(name: &str) -> Self {
        Self::new(
            DatasetSelection::new(vec![name.into()]).expect("a dataset name"),
            DatasetProducer::Mcap,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::episode::EpisodeFactsPresentation as _;
    use spiral_robotics_common::datasets::EpisodeId;

    fn lerobot() -> EpisodeListing {
        EpisodeListing::new(
            DatasetSelection::new(vec!["lerobot_svla_so101_pickplace".into()]).unwrap(),
            DatasetProducer::LeRobot,
        )
    }

    #[test]
    fn a_page_reads_in_recording_order_and_bad_rows_are_errors() {
        let mcap = EpisodeListing::mcap_fixture("recordings");
        assert!(mcap.episode(Default::default()).is_err());
        let named = serde_json::json!({ "id": "Episode 7" });
        assert!(mcap.episode(named.as_object().unwrap().clone()).is_err());
        let owned = EpisodeListing::mcap_fixture("owner's recordings");
        let sql = owned.page_sql(50, 50);
        assert!(sql.contains("spql.mcap.episodes('owner''s recordings')"));
        assert!(sql.ends_with("ORDER BY message_start_time, source_uri, id LIMIT 51 OFFSET 50"));
    }

    /// The columns `spql.mcap.episodes` has for a LeRobot dataset, and no
    /// other: no channel count, and a source that is not the episode's own.
    #[test]
    fn a_lerobot_dataset_lists_its_index_and_tasks_in_its_own_order() {
        let listing = lerobot();
        assert_eq!(
            listing.first_sql(100),
            "SELECT id, episode_index, tasks, message_count, message_start_time, duration_ns \
             FROM spql.mcap.episodes('lerobot_svla_so101_pickplace') ORDER BY episode_index, id LIMIT 100"
        );
        assert!(!listing.sql().contains("channel_count") && !listing.sql().contains("source"));
        let row = serde_json::json!({
            "id": "01a1181263837279a03b89d481c7ed9b", "episode_index": 0,
            "tasks": ["pink lego brick into the transparent box"], "message_count": 1212,
            "message_start_time": 0, "duration_ns": 10_066_666_603_u64,
        });
        let episode = listing.episode(row.as_object().unwrap().clone()).unwrap();
        let facts = episode.facts();
        assert_eq!(facts.episode_index(), Some(0));
        assert_eq!(facts.tasks(), ["pink lego brick into the transparent box"]);
        assert_eq!((facts.channel_count(), facts.source_bytes()), (None, None));
        assert_eq!(
            facts.facts_label(),
            "Recorded without a wall clock · 1,212 messages · pink lego brick into the transparent box"
        );
        assert_eq!(episode.listing(), &listing);
        assert!(episode.matches("LEGO") && !episode.matches("cloth"));
    }

    #[test]
    fn rows_beside_the_listing_carry_each_producers_facts() {
        let sql = lerobot().beside_sql("SELECT episode_id FROM kept", "listed_");
        assert!(sql.starts_with(
            "SELECT named.*, listed.episode_index AS listed_episode_index, listed.tasks AS listed_tasks,"
        ));
        let row = serde_json::json!({
            "episode_id": EpisodeId::fixture(3), "listed_episode_index": 3,
            "listed_tasks": ["fold"], "listed_message_start_time": 0, "listed_duration_ns": 5,
        });
        let episode = lerobot()
            .beside(row.as_object().unwrap(), "listed_")
            .unwrap();
        assert_eq!(episode.id(), &EpisodeId::fixture(3));
        assert_eq!(episode.recording_place().0, Some(3));
    }

    /// A curation keeps its dataset with the producer, and one saved before
    /// producers were kept reads as MCAP recordings.
    #[test]
    fn a_curation_keeps_the_producer_beside_the_dataset() {
        let json = serde_json::to_value(lerobot()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"path": ["lerobot_svla_so101_pickplace"], "producer": "spql.lerobot.dataset"})
        );
        let saved: EpisodeListing =
            serde_json::from_value(serde_json::json!({"path": ["dataset_abc130k"]})).unwrap();
        assert_eq!(saved, EpisodeListing::mcap_fixture("dataset_abc130k"));
    }
}
