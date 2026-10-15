//! A dataset's `topics` table, one row per recorded stream
//! ([datasets](../../../../../docs/datasets.md#topics)), and what reading it
//! reports: the dataset's inspection, and the topics one episode holds.
use super::EpisodeTopicQuery;
use super::writer::DecodedEpisode;
use crate::db::{ArrowColumns, DatabaseSession as _};
use crate::{ProjectConnection, StorageManager};
use arrow_array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetSelection, DatasetTopic, EpisodeSelection, TopicRevision,
};
use spiral_robotics_common::sql::ListItems;
use std::collections::BTreeMap;
use std::sync::Arc;

/// What the table says of one stream.
#[derive(Clone, Debug, PartialEq)]
struct TopicRow {
    topic: String,
    schema: Option<String>,
    encoding: String,
    episodes: u64,
    messages: u64,
}

impl TopicRow {
    /// Row `row` of a read of the table's columns; none where `stream` is
    /// NULL, as a dataset without streams reads.
    fn read(
        columns: &ArrowColumns<'_>,
        row: usize,
    ) -> Result<Option<(TopicRevision, Self)>, String> {
        let Some(stream) = columns.optional_text("stream", row)? else {
            return Ok(None);
        };
        let count = |name: &str| {
            columns
                .optional_unsigned(name, row)?
                .ok_or_else(|| format!("stored topics row has a null {name}"))
        };
        Ok(Some((
            TopicRevision::new(stream)?,
            Self {
                topic: columns.text("topic", row)?.to_owned(),
                schema: columns
                    .optional_text("schema_name", row)?
                    .map(str::to_owned),
                encoding: columns.text("message_encoding", row)?.to_owned(),
                episodes: count("episodes")?,
                messages: count("messages")?,
            },
        )))
    }

    fn topic(&self, stream: &TopicRevision) -> DatasetTopic {
        DatasetTopic::new(self.topic.clone(), stream.clone(), self.encoding.clone())
            .with_schema(self.schema.clone())
            .with_coverage(self.episodes, self.messages)
    }
}

/// The table's rows: each stream the dataset records, with how many
/// episodes and messages hold it.
#[derive(Default)]
pub(crate) struct TopicCounts {
    rows: BTreeMap<TopicRevision, TopicRow>,
}

impl TopicCounts {
    /// The rows `dataset`'s table holds now.
    pub(crate) async fn read(
        connection: &mut ProjectConnection,
        dataset: &DatasetSelection,
    ) -> Result<Self, String> {
        let batches = connection
            .read_batches(&format!(
                "SELECT stream, topic, schema_name, message_encoding, episodes, messages FROM {}",
                dataset.topics()
            ))
            .await?;
        let mut rows = BTreeMap::new();
        for batch in &batches {
            let columns = ArrowColumns::new(batch, "topics");
            for row in 0..batch.num_rows() {
                rows.extend(TopicRow::read(&columns, row)?);
            }
        }
        Ok(Self { rows })
    }

    /// Count `episode`'s messages of each stream it holds.
    pub(crate) fn count(&mut self, episode: &DecodedEpisode) -> Result<(), String> {
        for messages in episode.streams() {
            let stream = messages.stream();
            let row = self
                .rows
                .entry(stream.name().clone())
                .or_insert_with(|| TopicRow {
                    topic: stream.topic().to_owned(),
                    schema: stream.schema().map(str::to_owned),
                    encoding: stream.encoding().to_owned(),
                    episodes: 0,
                    messages: 0,
                });
            if row.topic != stream.topic()
                || row.schema.as_deref() != stream.schema()
                || row.encoding != stream.encoding()
            {
                return Err(format!(
                    "{} and {} share the stream name {}",
                    row.topic,
                    stream.topic(),
                    stream.name().stream()
                ));
            }
            row.episodes += 1;
            row.messages += messages.len() as u64;
        }
        Ok(())
    }

    /// Replace `dataset`'s table with these rows.
    pub(crate) async fn replace(
        &self,
        connection: &mut ProjectConnection,
        dataset: &DatasetSelection,
    ) -> Result<(), String> {
        connection
            .replace_or_create_batches(dataset.topics().name(), vec![self.batch()?])
            .await
    }

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("stream", DataType::Utf8, false),
            Field::new("topic", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, true),
            Field::new("message_encoding", DataType::Utf8, false),
            Field::new("episodes", DataType::UInt64, false),
            Field::new("messages", DataType::UInt64, false),
        ]))
    }

    fn batch(&self) -> Result<RecordBatch, String> {
        let rows = || self.rows.iter();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows().map(|(stream, _)| stream.stream()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows().map(|(_, row)| row.topic.as_str()),
            )),
            Arc::new(StringArray::from_iter(
                rows().map(|(_, row)| row.schema.as_deref()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows().map(|(_, row)| row.encoding.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows().map(|(_, row)| row.episodes),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows().map(|(_, row)| row.messages),
            )),
        ];
        RecordBatch::try_new(Self::schema(), columns)
            .map_err(|error| format!("building topics rows: {error}"))
    }

    /// Every stream as the dataset's topics, by topic.
    pub(super) fn topics(&self) -> Vec<DatasetTopic> {
        let mut topics = self
            .rows
            .iter()
            .map(|(stream, row)| row.topic(stream))
            .collect::<Vec<_>>();
        topics.sort_by(|left, right| left.name().cmp(right.name()));
        topics
    }
}

impl StorageManager {
    /// `selection` as the catalog names it now, how many episodes it holds,
    /// and every stream it records, read in one statement.
    pub async fn inspect_dataset(
        &self,
        selection: DatasetSelection,
    ) -> Result<DatasetInspection, String> {
        let dataset = self
            .project_store()
            .dataset_catalog()
            .datasets()
            .await?
            .summary(&selection)
            .ok_or("Dataset no longer exists in this project; refresh the catalog")?;
        let batches = self
            .project_store()
            .connect()
            .await?
            .read_batches(&format!(
                "SELECT t.stream, t.topic, t.schema_name, t.message_encoding, t.episodes, t.messages, \
                 e.episode_count FROM (SELECT count(*) AS episode_count FROM {}) e \
                 LEFT JOIN {} t ON TRUE",
                selection.episodes(),
                selection.topics()
            ))
            .await?;
        let mut episodes = 0;
        let mut counts = TopicCounts::default();
        for batch in &batches {
            let columns = ArrowColumns::new(batch, "dataset inspection");
            for row in 0..batch.num_rows() {
                episodes = columns
                    .optional_unsigned("episode_count", row)?
                    .unwrap_or_default();
                counts.rows.extend(TopicRow::read(&columns, row)?);
            }
        }
        Ok(DatasetInspection::new(dataset, episodes, counts.topics()))
    }

    /// The dataset's topics `episode` holds messages of, each with how
    /// many, read from its row of `episodes` in one statement. An episode
    /// the dataset does not hold holds none.
    pub async fn episode_topics(
        &self,
        episode: EpisodeSelection,
    ) -> Result<Vec<DatasetTopic>, String> {
        let dataset = episode.dataset();
        let statement = EpisodeTopicQuery::for_episode(
            episode.episode_id(),
            format!(
                "SELECT t.stream, t.topic, t.schema_name, t.message_encoding, u.s.messages AS messages \
                 FROM {} u JOIN {} t ON t.stream = u.s.stream",
                ListItems::of(
                    format!("(SELECT streams FROM {} WHERE id = $1)", dataset.episodes()),
                    "streams",
                    "s"
                ),
                dataset.topics()
            ),
        )?;
        let mut connection = self.project_store().connect().await?;
        let mut rows = connection
            .query(statement.into_statement())
            .await
            .map_err(|error| error.to_string())?;
        let mut topics = Vec::new();
        while let Some(batch) = rows.next().await {
            let batch = batch.map_err(|error| error.to_string())?;
            let columns = ArrowColumns::new(&batch, "episode topics");
            for row in 0..batch.num_rows() {
                let stream = TopicRevision::new(columns.text("stream", row)?)?;
                let messages = columns
                    .optional_unsigned("messages", row)?
                    .ok_or("stored episode stream has a null message count")?;
                topics.push(
                    DatasetTopic::new(
                        columns.text("topic", row)?.to_owned(),
                        stream,
                        columns.text("message_encoding", row)?.to_owned(),
                    )
                    .with_schema(
                        columns
                            .optional_text("schema_name", row)?
                            .map(str::to_owned),
                    )
                    .with_coverage(1, messages),
                );
            }
        }
        topics.sort_by(|left, right| {
            (left.name(), left.revision()).cmp(&(right.name(), right.revision()))
        });
        Ok(topics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::fixtures::RolloutFixture;
    use crate::datasets::{DatasetImport, EpisodeUuids, McapFile, TestJournal};
    use crate::db::MemoryDatabase;
    use spiral_robotics_common::datasets::{DatasetFormat, EpisodeId};

    #[tokio::test]
    async fn a_dataset_and_each_episode_list_the_streams_they_hold() {
        let journal = TestJournal::start().await;
        let (store, _) = MemoryDatabase::store_with_catalog(journal.endpoint()).await;
        let store = Arc::new(store);
        let storage = StorageManager::new(
            store.clone(),
            crate::StudioStore::in_memory().project(&store.connection_settings()),
        );
        let dataset = DatasetSelection::new("demo").unwrap();
        let episodes = [("filmed", true), ("blind", false)].map(|(key, filmed)| {
            let bytes = RolloutFixture::mcap(key, filmed);
            DecodedEpisode::of_mcap(McapFile::held(key, &bytes).unwrap(), &bytes).unwrap()
        });
        DatasetImport::new(&store, &dataset, DatasetFormat::Mcap)
            .commit(episodes.into())
            .await
            .unwrap();
        journal.name(&[dataset.episodes(), dataset.topics(), dataset.files()]);

        let inspection = storage.inspect_dataset(dataset.clone()).await.unwrap();
        assert_eq!(inspection.members(), 2);
        assert_eq!(inspection.format(), DatasetFormat::Mcap);
        let camera = inspection
            .topics()
            .iter()
            .find(|topic| topic.is_video())
            .unwrap();
        assert_eq!((camera.name(), camera.members()), ("/camera/top/video", 1));

        let batches = store
            .connect()
            .await
            .unwrap()
            .read_batches(&format!(
                "SELECT id FROM {} WHERE key = 'filmed'",
                dataset.episodes()
            ))
            .await
            .unwrap();
        let filmed = EpisodeUuids::read(batches[0].column(0).as_ref()).unwrap()[0].clone();
        let topics = storage
            .episode_topics(EpisodeSelection::new(dataset.clone(), filmed))
            .await
            .unwrap();
        assert_eq!(topics.len(), inspection.topics().len());
        let held = topics.iter().find(|topic| topic.is_video()).unwrap();
        assert_eq!(
            (held.revision(), held.members(), held.samples()),
            (camera.revision(), 1, 8)
        );
        let stranger = EpisodeId::try_from("01900000000070008000000000000001").unwrap();
        assert!(
            storage
                .episode_topics(EpisodeSelection::new(dataset.clone(), stranger))
                .await
                .unwrap()
                .is_empty()
        );
        let empty = DatasetSelection::new("empty").unwrap();
        assert!(
            storage.inspect_dataset(empty).await.is_err(),
            "no such dataset"
        );
    }
}
