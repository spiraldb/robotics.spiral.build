//! Each episode's row: the episode's first message, which every stored time
//! counts from, and what the nodes of a path ask a model about the whole
//! episode, one column each, which their entries read.
use super::{PathError, PathSource};
use crate::activities::curation::graph::DatasetStep;
use crate::activities::episode::EpisodeTask;
use spiral_robotics_common::curation::{AnswerColumn, AnswerTable};
use spiral_robotics_common::datasets::{
    CameraVideo, DatasetSelection, DatasetStreams, TopicRevision,
};

/// How a row comes by one answer: read from the dataset's answers table, or
/// asked of the model by a call on the row itself.
#[derive(Clone, Debug)]
enum RowAnswer {
    Stored,
    Asked(String),
}

/// Each episode's row: every answer the nodes ask a model about the whole
/// episode, one column each, named for the answers table column that stores
/// it. A stored answer is read from the answers table; one asked on the row
/// reads the camera videos and the instructions its call names, each from
/// its stream among the dataset's, a camera two models watch read once, and
/// no video leaves the row. Published, the row is built on the dataset's
/// listing, one per episode; previewed, it is the listed episodes'; filled,
/// the bound episode's alone.
pub(in crate::activities::curation) struct EpisodeRow<'a> {
    dataset: DatasetSelection,
    source: PathSource,
    /// The dataset's streams, among which a fill finds what its call reads.
    streams: Option<DatasetStreams<'a>>,
    cameras: Vec<TopicRevision>,
    instructions: Vec<TopicRevision>,
    answers: Vec<(AnswerColumn, RowAnswer)>,
}

impl<'a> EpisodeRow<'a> {
    /// The column the episode's first message rides in, in log time.
    pub(in crate::activities::curation) const ORIGIN: &'static str = "origin_ns";
    /// The entry holding each episode's row.
    pub(in crate::activities::curation) const EPISODES: &'static str = "episodes";
    /// The entries holding each episode's video of one camera, numbered.
    const VIDEOS: &'static str = "camera_video";
    /// The entries holding each episode's instruction on one topic, numbered.
    const INSTRUCTIONS: &'static str = "instruction";
    /// What the answers table is read as.
    const STORED: &'static str = "a";
    /// What the dataset's listing of the bound episode is read as.
    const LISTED: &'static str = "e";

    /// The row a published view or a preview reads, of stored answers.
    pub(in crate::activities::curation) fn new(
        dataset: &DatasetSelection,
        source: PathSource,
    ) -> Self {
        Self {
            dataset: dataset.clone(),
            source,
            streams: None,
            cameras: Vec::new(),
            instructions: Vec::new(),
            answers: Vec::new(),
        }
    }
    /// The row a fill asks its call on, reading what the call names among
    /// `streams`, the dataset's.
    pub(in crate::activities::curation) fn filling(
        dataset: &DatasetSelection,
        streams: DatasetStreams<'a>,
    ) -> Self {
        Self {
            streams: Some(streams),
            ..Self::new(dataset, PathSource::Fill)
        }
    }

    /// Which reading of the path the row is built for.
    pub(in crate::activities::curation) fn source(&self) -> PathSource {
        self.source
    }
    /// The episode's packet video of the camera topic `camera`.
    pub(in crate::activities::curation) fn video(
        &mut self,
        camera: &str,
    ) -> Result<String, PathError> {
        let stream = self.recorded()?.camera(camera).map_err(PathError::new)?;
        let entry = Self::entry(
            Self::VIDEOS,
            Self::read_once(&mut self.cameras, stream.revision()),
        );
        Ok(format!("{entry}.source"))
    }
    /// The episode's task, where its dataset records it: the first message
    /// on an instruction topic, or the first task its dataset lists.
    pub(in crate::activities::curation) fn task(
        &mut self,
        task: &EpisodeTask,
    ) -> Result<String, PathError> {
        match task {
            EpisodeTask::Recorded(topic) => {
                let stream = self.recorded()?.topic(topic).map_err(PathError::new)?;
                let entry = Self::entry(
                    Self::INSTRUCTIONS,
                    Self::read_once(&mut self.instructions, stream.revision()),
                );
                Ok(format!("{entry}.task"))
            }
            EpisodeTask::Listed => Ok(format!("spql.list.get({}.tasks, 0)", Self::LISTED)),
        }
    }
    /// The streams a fill reads from: only a fill reads recordings.
    fn recorded(&self) -> Result<DatasetStreams<'a>, PathError> {
        self.streams
            .ok_or_else(|| PathError::new("Only a fill reads an episode's recordings."))
    }
    /// Read each episode's answers in `column` from the answers table.
    pub(in crate::activities::curation) fn stored(
        &mut self,
        column: &AnswerColumn,
    ) -> Result<(), PathError> {
        self.answer(column, RowAnswer::Stored)
    }
    /// Ask each episode for the answers `column` stores, by `call`.
    pub(in crate::activities::curation) fn asked(
        &mut self,
        column: &AnswerColumn,
        call: String,
    ) -> Result<(), PathError> {
        self.answer(column, RowAnswer::Asked(call))
    }

    /// The row's entries, the last of them [`Self::EPISODES`]; nothing when
    /// no node asks anything. Published, each episode's first message is the
    /// start of its listed span.
    pub(in crate::activities::curation) fn entries(self) -> Vec<String> {
        if self.answers.is_empty() {
            return Vec::new();
        }
        let mut entries = self.reads();
        entries.push(match self.source {
            PathSource::Published => format!(
                "{} AS (SELECT d.episode_id, d.{origin}{} FROM {} d{})",
                Self::EPISODES,
                self.answer_columns(),
                DatasetStep::NAME,
                self.joins("d.episode_id"),
                origin = Self::ORIGIN,
            ),
            PathSource::Preview { .. } | PathSource::Fill => self.bound_episodes(),
        });
        entries
    }

    /// The one statement filling the one answer the row asks for, for the
    /// episode bound as `$1`: `episode_id` and its `answer`, or no row for an
    /// episode without what the call reads.
    pub(in crate::activities::curation) fn answering(self) -> Result<String, PathError> {
        let [(column, _)] = self.answers.as_slice() else {
            return Err(PathError::new("A fill asks each episode one thing."));
        };
        let column = column.clone();
        let entries = self.entries();
        Ok(format!(
            "WITH\n  {}\nSELECT episode_id, {column} AS answer FROM {}",
            entries.join(",\n  "),
            Self::EPISODES
        ))
    }

    /// Two nodes making one call on a path read its one stored column; one
    /// asked inline is asked once.
    fn answer(&mut self, column: &AnswerColumn, answer: RowAnswer) -> Result<(), PathError> {
        match self.answers.iter().find(|(asked, _)| asked == column) {
            None => {
                self.answers.push((column.clone(), answer));
                Ok(())
            }
            Some((_, RowAnswer::Stored)) if matches!(answer, RowAnswer::Stored) => Ok(()),
            Some(_) => Err(PathError::new(format!(
                "Two nodes of this path ask each episode for its {column}."
            ))),
        }
    }

    /// The bound episodes' rows, read from the dataset's listing: the one a
    /// fill binds as `$1`, or every one a preview lists, `$1` through `$N`.
    fn bound_episodes(&self) -> String {
        let listed = Self::LISTED;
        let members = match self.source {
            PathSource::Preview { episodes } => {
                let bound = (1..=episodes)
                    .map(|place| format!("${place}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{listed}.id IN ({bound})")
            }
            PathSource::Published | PathSource::Fill => format!("{listed}.id = $1"),
        };
        format!(
            "{} AS (SELECT {listed}.id AS episode_id, CAST({listed}.message_start_time AS BIGINT) AS {}{} \
             FROM {} {listed}{} WHERE {members})",
            Self::EPISODES,
            Self::ORIGIN,
            self.answer_columns(),
            self.dataset.episodes(),
            self.joins(&format!("{listed}.id"))
        )
    }

    /// Every answer as a column of the row, led by a comma.
    fn answer_columns(&self) -> String {
        self.answers
            .iter()
            .map(|(column, answer)| match answer {
                RowAnswer::Stored => format!(", {}.{column}.value AS {column}", Self::STORED),
                RowAnswer::Asked(call) => format!(", {call} AS {column}"),
            })
            .collect()
    }

    /// The entries reading every camera video and instruction the row asks
    /// with, of the episode bound as `$1`: only a fill asks on the row.
    fn reads(&self) -> Vec<String> {
        let videos = self.cameras.iter().enumerate().map(|(index, camera)| {
            let videos = Self::entry(Self::VIDEOS, index);
            CameraVideo::new(&self.dataset, camera).videos((&format!("{videos}_packets"), &videos))
        });
        let instructions = self.instructions.iter().enumerate().map(|(index, stream)| {
            format!(
                "{} AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, \
                 ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time, r.ordinal) AS position \
                 FROM {} r WHERE r.id = $1) instructions WHERE position = 1)",
                Self::entry(Self::INSTRUCTIONS, index),
                self.dataset.messages(stream)
            )
        });
        videos.chain(instructions).collect()
    }

    /// The answers table joined to the episode `id` names when an answer is
    /// stored, every video the row reads, and every instruction beside it:
    /// an episode without the camera has no answer, and one without an
    /// instruction is asked without a task.
    fn joins(&self, id: &str) -> String {
        let stored = self
            .answers
            .iter()
            .any(|(_, answer)| matches!(answer, RowAnswer::Stored));
        let answers = if stored {
            let alias = Self::STORED;
            format!(
                " JOIN {} {alias} ON {alias}.episode_id = {id}",
                AnswerTable::of(&self.dataset).table()
            )
        } else {
            String::new()
        };
        let videos = (0..self.cameras.len()).map(|index| {
            let entry = Self::entry(Self::VIDEOS, index);
            format!(" JOIN {entry} ON {entry}.id = {id}")
        });
        let instructions = (0..self.instructions.len()).map(|index| {
            let entry = Self::entry(Self::INSTRUCTIONS, index);
            format!(" LEFT JOIN {entry} ON {entry}.id = {id}")
        });
        std::iter::once(answers)
            .chain(videos)
            .chain(instructions)
            .collect()
    }

    fn entry(kind: &str, index: usize) -> String {
        format!("{kind}_{}", index + 1)
    }

    /// Where `read` is among `reads`, added the first time it is asked for.
    fn read_once(reads: &mut Vec<TopicRevision>, read: &TopicRevision) -> usize {
        match reads.iter().position(|existing| existing == read) {
            Some(index) => index,
            None => {
                reads.push(read.clone());
                reads.len() - 1
            }
        }
    }
}
