//! Writing decoded episodes into a dataset's tables
//! ([datasets](../../../../../docs/datasets.md#writing)): at most 32 at a
//! time, each batch every stream's message rows first, then the episodes'
//! rows, a row of the answers table per episode, and an MCAP episode's file;
//! the `topics` table is replaced with the new counts once, when the
//! transaction's writes are done. A reader that finds an episode therefore
//! finds all its messages.
use super::answers::AnswerRows;
use super::episodes::EpisodeRows;
use super::files::McapFile;
use super::streams::{RecordedStream, StreamMessages};
use super::topics::TopicCounts;
use crate::{ProjectConnection, ProjectStore};
use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use spiral_robotics_common::datasets::{DatasetFormat, DatasetSelection, EpisodeId, TopicRevision};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One episode decoded from its source, ready to write.
pub(crate) struct DecodedEpisode {
    key: String,
    origin: EpisodeOrigin,
    streams: Vec<StreamMessages>,
}

/// What an episode was read from.
pub(crate) enum EpisodeOrigin {
    /// An MCAP file, which the dataset's `files` keeps.
    Mcap(McapFile),
    /// An episode of a LeRobot repository.
    LeRobot(LeRobotEpisode),
}

/// What a LeRobot repository records about one of its episodes.
pub(crate) struct LeRobotEpisode {
    /// The repository's root at its commit.
    root: String,
    index: i64,
    tasks: Vec<String>,
}

impl LeRobotEpisode {
    pub(crate) fn new(root: String, index: i64, tasks: Vec<String>) -> Self {
        Self { root, index, tasks }
    }
    pub(crate) fn root(&self) -> &str {
        &self.root
    }
    pub(crate) fn index(&self) -> i64 {
        self.index
    }
    pub(crate) fn tasks(&self) -> &[String] {
        &self.tasks
    }
}

impl EpisodeOrigin {
    fn format(&self) -> DatasetFormat {
        match self {
            Self::Mcap(_) => DatasetFormat::Mcap,
            Self::LeRobot(_) => DatasetFormat::LeRobot,
        }
    }
    /// The MCAP file it was read from, if it was.
    pub(crate) fn mcap(&self) -> Option<&McapFile> {
        match self {
            Self::Mcap(file) => Some(file),
            Self::LeRobot(_) => None,
        }
    }
    /// The LeRobot episode it was read from, if it was.
    pub(crate) fn lerobot(&self) -> Option<&LeRobotEpisode> {
        match self {
            Self::LeRobot(episode) => Some(episode),
            Self::Mcap(_) => None,
        }
    }
}

impl DecodedEpisode {
    /// Episode `key` of its source, holding `streams`.
    pub(crate) fn new(key: String, origin: EpisodeOrigin, streams: Vec<StreamMessages>) -> Self {
        Self {
            key,
            origin,
            streams,
        }
    }
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
    pub(crate) fn origin(&self) -> &EpisodeOrigin {
        &self.origin
    }
    pub(crate) fn streams(&self) -> &[StreamMessages] {
        &self.streams
    }
    /// Its first and last message's log times, from 0 for a LeRobot episode,
    /// which is timed from its start; both its start without messages.
    pub(crate) fn time_span(&self) -> (u64, u64) {
        let span = self
            .streams
            .iter()
            .filter_map(StreamMessages::time_span)
            .reduce(|(first, last), (start, end)| (first.min(start), last.max(end)));
        let start = match (&self.origin, span) {
            (EpisodeOrigin::LeRobot(_), _) | (EpisodeOrigin::Mcap(_), None) => 0,
            (EpisodeOrigin::Mcap(_), Some((start, _))) => start,
        };
        (start, span.map_or(start, |(_, end)| end.max(start)))
    }
}

/// Writes one dataset's episodes within the caller's transaction. It knows
/// the dataset's topics and answer columns as of its last write, so a
/// transaction reads them at most once however many batches it writes.
pub(crate) struct DatasetWrite<'a> {
    dataset: &'a DatasetSelection,
    format: DatasetFormat,
    topics: TopicCounts,
    answers: SchemaRef,
}

impl<'a> DatasetWrite<'a> {
    /// The most episodes one write takes.
    pub(crate) const MAX_EPISODES: usize = 32;

    /// Create `dataset` in `format` with no episodes: empty episodes, topics
    /// and answers tables, and an MCAP dataset's empty files.
    pub(crate) async fn create(
        connection: &mut ProjectConnection,
        dataset: &'a DatasetSelection,
        format: DatasetFormat,
    ) -> Result<Self, String> {
        let write = Self {
            dataset,
            format,
            topics: TopicCounts::default(),
            answers: Arc::new(AnswerRows::schema()),
        };
        connection
            .create_or_append_batches(
                &dataset.episodes().target(),
                vec![RecordBatch::new_empty(EpisodeRows::schema())],
            )
            .await?;
        connection
            .create_or_append_batches(
                &dataset.answers().target(),
                vec![RecordBatch::new_empty(write.answers.clone())],
            )
            .await?;
        if format == DatasetFormat::Mcap {
            connection
                .create_or_append_batches(
                    &dataset.files().target(),
                    vec![RecordBatch::new_empty(McapFile::schema())],
                )
                .await?;
        }
        Ok(write)
    }

    /// `dataset`, in `format`, as the transaction sees it now.
    pub(crate) async fn open(
        connection: &mut ProjectConnection,
        dataset: &'a DatasetSelection,
        format: DatasetFormat,
    ) -> Result<Self, String> {
        let topics = TopicCounts::read(connection, dataset).await?;
        let answers = connection
            .relation_schema(&dataset.answers().target())
            .await?
            .ok_or_else(|| format!("{} has no answers table", dataset.label()))?;
        Ok(Self {
            dataset,
            format,
            topics,
            answers: Arc::new(answers),
        })
    }

    /// Write `episodes`, at most [`Self::MAX_EPISODES`], each of this
    /// dataset's format.
    pub(crate) async fn write(
        &mut self,
        connection: &mut ProjectConnection,
        episodes: Vec<DecodedEpisode>,
    ) -> Result<(), String> {
        if episodes.len() > Self::MAX_EPISODES {
            return Err(format!(
                "A write takes at most {} episodes, not {}",
                Self::MAX_EPISODES,
                episodes.len()
            ));
        }
        let mut streams = BTreeMap::<TopicRevision, (RecordedStream, Vec<RecordBatch>)>::new();
        let mut written = Vec::with_capacity(episodes.len());
        for episode in episodes {
            if episode.origin().format() != self.format {
                return Err(format!(
                    "{} cannot hold episode {}, read in another format",
                    self.dataset.label(),
                    episode.key()
                ));
            }
            // Every episode Studio writes gets its identity here, once.
            let id = EpisodeId::from(*uuid::Uuid::now_v7().as_bytes());
            for messages in episode.streams() {
                let (stream, batches) = streams
                    .entry(messages.stream().name().clone())
                    .or_insert_with(|| (messages.stream().clone(), Vec::new()));
                if stream != messages.stream() {
                    return Err(format!(
                        "{} and {} share the stream name {}",
                        stream.topic(),
                        messages.stream().topic(),
                        stream.name().stream()
                    ));
                }
                batches.push(messages.batch(&id)?);
            }
            written.push((id, episode));
        }
        for (name, (_, batches)) in streams {
            connection
                .create_or_append_batches(&self.dataset.messages(&name).target(), batches)
                .await?;
        }
        connection
            .create_or_append_batches(
                &self.dataset.episodes().target(),
                vec![EpisodeRows::batch(&written)?],
            )
            .await?;
        for (_, episode) in &written {
            self.topics.count(episode)?;
        }
        let ids = written.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        connection
            .create_or_append_batches(
                &self.dataset.answers().target(),
                vec![AnswerRows::batch(&self.answers, &ids)?],
            )
            .await?;
        let files = written
            .iter()
            .filter_map(|(_, episode)| Some(episode.origin().mcap()?.row().clone()))
            .collect::<Vec<_>>();
        connection
            .create_or_append_batches(&self.dataset.files().target(), files)
            .await
    }

    /// Replace the `topics` table with the counts every write of this
    /// transaction left. Once per transaction: Spiral refuses to replace a
    /// table the transaction already created or replaced.
    pub(crate) async fn finish(self, connection: &mut ProjectConnection) -> Result<(), String> {
        self.topics.replace(connection, self.dataset).await
    }
}

/// A new dataset filled batch by batch, each batch in a transaction of its
/// own: a failed batch commits nothing, and the batches before it stay.
pub(crate) struct DatasetImport<'a> {
    store: &'a ProjectStore,
    dataset: &'a DatasetSelection,
    format: DatasetFormat,
    created: bool,
}

impl<'a> DatasetImport<'a> {
    /// An import into `dataset`, which does not exist yet.
    pub(crate) fn new(
        store: &'a ProjectStore,
        dataset: &'a DatasetSelection,
        format: DatasetFormat,
    ) -> Self {
        Self {
            store,
            dataset,
            format,
            created: false,
        }
    }

    /// Commit `episodes`, at most [`DatasetWrite::MAX_EPISODES`], creating
    /// the dataset with the first batch.
    pub(crate) async fn commit(&mut self, episodes: Vec<DecodedEpisode>) -> Result<(), String> {
        let Self {
            store,
            dataset,
            format,
            created,
        } = self;
        let mut connection = store.connect_exclusive().await?;
        connection
            .transaction("dataset import", async |connection| {
                let mut write = if *created {
                    DatasetWrite::open(connection, dataset, *format).await?
                } else {
                    DatasetWrite::create(connection, dataset, *format).await?
                };
                write.write(connection, episodes).await?;
                write.finish(connection).await
            })
            .await?;
        *created = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
