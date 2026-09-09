//! Topic membership from one episode's MCAP channel summary.
use std::collections::{BTreeMap, BTreeSet};

use super::EpisodeTopicQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeSelection};

impl StorageManager {
    pub async fn episode_topics(
        &self,
        episode: EpisodeSelection,
    ) -> Result<Vec<DatasetTopic>, String> {
        let statement = EpisodeTopicQuery::channels(&episode)?.statement;
        let mut connection = self.project_store().connect().await?;
        let mut stream = connection
            .query(statement)
            .await
            .map_err(|error| error.to_string())?;
        let mut found_episode = false;
        let mut topics = BTreeMap::<String, TopicChannels>::new();
        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|error| error.to_string())?;
            found_episode |= batch.num_rows() > 0;
            let columns = ArrowColumns::new(&batch, "episode channels");
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
        Ok(topics
            .into_iter()
            .map(|(name, channels)| channels.into_topic(name))
            .collect())
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

    fn into_topic(self, name: String) -> DatasetTopic {
        DatasetTopic::new(name, self.schemas.into_iter().collect()).with_coverage(1, self.samples)
    }
}

impl EpisodeTopicQuery {
    fn channels(episode: &EpisodeSelection) -> Result<Self, String> {
        Self::for_episode(
            episode.dataset(),
            episode.episode_id(),
            format!(
                "SELECT c.topic, c.schema_name, c.message_count \
                 FROM (SELECT source FROM spql.mcap.episodes('{}') WHERE id = $1) AS e \
                 LEFT JOIN LATERAL spql.mcap.channels(e.source) AS c ON TRUE",
                episode.dataset().sql_name().replace('\'', "''"),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
