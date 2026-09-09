//! Topic membership within one native episode, without downloading message payloads.
use super::EpisodeTopicQuery;
use crate::{StorageManager, db::DatabaseSession};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{DatasetTopic, EpisodeSelection};

impl StorageManager {
    pub async fn episode_topics(
        &self,
        episode: EpisodeSelection,
    ) -> Result<Vec<DatasetTopic>, String> {
        let membership = EpisodeTopicQuery::for_episode(
            episode.dataset(),
            episode.episode_id(),
            format!(
                "SELECT id FROM spql.mcap.episodes('{}') WHERE id = $1 LIMIT 1",
                episode.dataset().sql_name().replace('\'', "''")
            ),
        )?;
        let member = crate::db::ProjectQueries::new(self.project_store())
            .query_with_parameters(membership.statement, 1)
            .await?;
        if member.rows().is_empty() {
            return Err("This episode is no longer in the dataset".into());
        }
        let inspection = self.inspect_dataset(episode.dataset().clone()).await?;
        let mut connection = self.project_store().connect().await?;
        let mut topics = Vec::new();
        for topic in inspection.topics() {
            let statement = EpisodeTopicQuery::topic_presence(&episode, topic.name())?.statement;
            let mut stream = connection
                .query(statement)
                .await
                .map_err(|error| error.to_string())?;
            while let Some(batch) = stream.next().await {
                if batch.map_err(|error| error.to_string())?.num_rows() > 0 {
                    topics.push(
                        DatasetTopic::new(topic.name().to_owned(), topic.schemas().to_vec())
                            .with_coverage(1, None),
                    );
                    break;
                }
            }
        }
        Ok(topics)
    }
}

impl EpisodeTopicQuery {
    fn topic_presence(episode: &EpisodeSelection, topic: &str) -> Result<Self, String> {
        Self::for_episode(
            episode.dataset(),
            episode.episode_id(),
            format!(
                "SELECT id FROM spql.mcap.read_topic('{}', '{}') WHERE id = $1 LIMIT 1",
                episode.dataset().sql_name().replace('\'', "''"),
                topic.replace('\'', "''")
            ),
        )
    }
}
