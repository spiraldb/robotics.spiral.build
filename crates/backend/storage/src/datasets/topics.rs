//! The topics one episode records, and how many messages each holds, read
//! the way the dataset's producer allows: an MCAP member's channel summary,
//! or, for a LeRobot member, its messages of each of the dataset's topics.
use std::collections::{BTreeMap, BTreeSet};

use super::EpisodeTopicQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetProducer, DatasetTopic, EpisodeSelection,
};
use spiral_robotics_common::sql::SqlLiteral;

impl StorageManager {
    pub async fn episode_topics(
        &self,
        episode: EpisodeSelection,
    ) -> Result<Vec<DatasetTopic>, String> {
        let catalog = self.inspect_dataset(episode.dataset().clone()).await?;
        let statement = match catalog.producer() {
            DatasetProducer::Mcap => EpisodeTopicQuery::channels(&episode)?,
            DatasetProducer::LeRobot => EpisodeTopicQuery::messages(&episode, &catalog)?,
        };
        let topics = self.member_topics(statement).await?;
        Ok(catalog
            .topics()
            .iter()
            .filter(|topic| topics.contains_key(topic.name()))
            .map(|topic| {
                let coverage = &topics[topic.name()];
                let variants = catalog
                    .topics()
                    .iter()
                    .filter(|candidate| candidate.name() == topic.name())
                    .count();
                topic.clone().with_coverage(
                    1,
                    if variants == 1 {
                        coverage.samples
                    } else {
                        None
                    },
                )
            })
            .collect())
    }

    /// The topics `statement` lists for its episode — `topic`, an optional
    /// `schema_name`, and `message_count` per row — merged by topic. An
    /// episode the statement lists no row for is not in the dataset.
    async fn member_topics(
        &self,
        statement: EpisodeTopicQuery,
    ) -> Result<BTreeMap<String, TopicChannels>, String> {
        let mut connection = self.project_store().connect().await?;
        let mut stream = connection
            .query(statement.into_statement())
            .await
            .map_err(|error| error.to_string())?;
        let mut found_episode = false;
        let mut topics = BTreeMap::<String, TopicChannels>::new();
        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|error| error.to_string())?;
            found_episode |= batch.num_rows() > 0;
            let columns = ArrowColumns::new(&batch, "episode topics");
            for row in 0..batch.num_rows() {
                let Some(topic) = columns.optional_text("topic", row)? else {
                    continue;
                };
                topics.entry(topic.to_owned()).or_default().include(
                    columns.optional_text("schema_name", row)?,
                    columns.optional_unsigned("message_count", row)?,
                )?;
            }
        }
        if !found_episode {
            return Err("This episode is no longer in the dataset".into());
        }
        Ok(topics)
    }
}

struct TopicChannels {
    schemas: BTreeSet<String>,
    samples: Option<u64>,
}

impl Default for TopicChannels {
    fn default() -> Self {
        Self {
            schemas: BTreeSet::new(),
            samples: Some(0),
        }
    }
}

impl TopicChannels {
    fn include(&mut self, schema: Option<&str>, samples: Option<u64>) -> Result<(), String> {
        if let Some(schema) = schema {
            self.schemas.insert(schema.to_owned());
        }
        self.samples = match (self.samples, samples) {
            (Some(total), Some(count)) => Some(
                total
                    .checked_add(count)
                    .ok_or("Episode topic message count exceeds u64")?,
            ),
            _ => None,
        };
        Ok(())
    }

    #[cfg(test)]
    fn into_topic(self, name: String) -> DatasetTopic {
        DatasetTopic::new(name, self.schemas.into_iter().collect()).with_coverage(1, self.samples)
    }
}

impl EpisodeTopicQuery {
    /// The MCAP member's channels, read from its file's summary.
    fn channels(episode: &EpisodeSelection) -> Result<Self, String> {
        Self::for_episode(
            episode.dataset(),
            episode.episode_id(),
            format!(
                "SELECT c.topic, c.schema_name, c.message_count \
                 FROM (SELECT source FROM spql.mcap.episodes({}) WHERE id = $1) AS e \
                 LEFT JOIN LATERAL spql.mcap.channels(e.source) AS c ON TRUE",
                SqlLiteral::text(&episode.dataset().sql_name()),
            ),
        )
    }

    /// How many messages the member holds of each topic the dataset
    /// records, the topics it holds none of left out. Spiral keeps no
    /// channel summary of a LeRobot member, whose source is a frame table
    /// shared with other episodes, so each topic's messages of the member
    /// are counted, all in one statement.
    fn messages(episode: &EpisodeSelection, catalog: &DatasetInspection) -> Result<Self, String> {
        let dataset = SqlLiteral::text(&episode.dataset().sql_name());
        let counts = catalog
            .topics()
            .iter()
            .map(|topic| {
                let variant = topic
                    .revision()
                    .map(|revision| format!(", {}", SqlLiteral::text(revision.variant())))
                    .unwrap_or_default();
                format!(
                    "SELECT {topic} AS topic, CAST(NULL AS VARCHAR) AS schema_name, count(*) AS message_count \
                     FROM spql.mcap.read_topic({dataset}, {topic}{variant}) WHERE id = $1",
                    topic = SqlLiteral::text(topic.name()),
                )
            })
            .collect::<Vec<_>>();
        if counts.is_empty() {
            return Err("This dataset records no topics".into());
        }
        Self::for_episode(
            episode.dataset(),
            episode.episode_id(),
            format!(
                "SELECT * FROM ({}) counted WHERE message_count > 0",
                counts.join(" UNION ALL ")
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::{
        DatasetSelection, DatasetSummary, EpisodeId, TopicRevision,
    };

    #[test]
    fn channels_merge_schemas_and_preserve_unknown_and_zero_counts() {
        let mut channels = TopicChannels::default();
        channels.include(Some("camera"), Some(2)).unwrap();
        channels.include(Some("camera"), Some(3)).unwrap();
        channels.include(Some("other"), Some(0)).unwrap();
        let topic = channels.into_topic("/camera".into());
        assert_eq!(topic.schemas(), ["camera", "other"]);
        assert_eq!(topic.samples(), Some(5));
        assert_eq!(topic.members(), 1);

        let mut channels = TopicChannels::default();
        channels.include(None, Some(0)).unwrap();
        assert_eq!(channels.samples, Some(0));
        channels.include(None, None).unwrap();
        channels.include(None, Some(2)).unwrap();
        assert_eq!(channels.into_topic("/raw".into()).samples(), None);
    }

    #[test]
    fn a_lerobot_member_counts_its_messages_of_every_topic_in_one_statement() {
        let dataset = DatasetSelection::new(vec!["lerobot_svla_so101_pickplace".into()]).unwrap();
        let catalog = DatasetInspection::new(
            DatasetSummary::new(dataset.clone(), ("r".into(), "v".into())),
            DatasetProducer::LeRobot,
            Some(50),
            ["action", "observation.images.up"]
                .map(|name| {
                    DatasetTopic::new(name.into(), vec![])
                        .with_revision(TopicRevision::new(format!("{name}-variant")))
                })
                .to_vec(),
        );
        let episode = EpisodeSelection::new(
            dataset,
            EpisodeId::try_from("01a1181263837279a03b89d481c7ed9b").unwrap(),
        );
        let statement = EpisodeTopicQuery::messages(&episode, &catalog)
            .unwrap()
            .into_statement();
        assert_eq!(
            statement.as_str(),
            "SELECT * FROM (SELECT 'action' AS topic, CAST(NULL AS VARCHAR) AS schema_name, count(*) AS message_count \
             FROM spql.mcap.read_topic('lerobot_svla_so101_pickplace', 'action', 'action-variant') WHERE id = $1 \
             UNION ALL SELECT 'observation.images.up' AS topic, CAST(NULL AS VARCHAR) AS schema_name, count(*) AS message_count \
             FROM spql.mcap.read_topic('lerobot_svla_so101_pickplace', 'observation.images.up', 'observation.images.up-variant') WHERE id = $1) \
             counted WHERE message_count > 0"
        );
        assert!(statement.parameters().is_some());
    }
}
