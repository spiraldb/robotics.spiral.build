//! Each episode's row: what the nodes of a path ask a model about the whole
//! episode, built once where the path still holds one row per episode, and
//! the answers that then ride on the interval rows below it until the step
//! that unnests each.
use super::{PathError, PathSource, Relation, StepSql};
use spiral_robotics_common::curation::{AnswerColumn, AnswerTable};
use spiral_robotics_common::datasets::{CameraVideo, DatasetSelection};
use spiral_robotics_common::sql::SqlLiteral;

/// How a row comes by one answer: read from the dataset's answers table, or
/// asked of the model by a call on the row itself.
#[derive(Clone, Debug)]
enum RowAnswer {
    Stored,
    Asked(String),
}

/// Each episode's row, where a path still holds one row per episode: every
/// answer the nodes ask a model about the whole episode, one column each,
/// named for the answers table column that stores it. A stored answer is
/// read from the answers table; one asked on the row reads the camera videos
/// and the instructions its call names, a camera two models watch read once,
/// and no video leaves the row. Every answer counts from the episode's first
/// message, which the row carries beside them.
pub(in crate::activities::curation) struct EpisodeRow {
    dataset: DatasetSelection,
    source: PathSource,
    cameras: Vec<String>,
    instructions: Vec<String>,
    answers: Vec<(AnswerColumn, RowAnswer)>,
}

impl EpisodeRow {
    /// The column the episode's first message rides in, in log time.
    pub(in crate::activities::curation) const ORIGIN: &'static str = "origin_ns";
    /// The entry holding each episode's row.
    const EPISODES: &'static str = "episodes";
    /// The entry in which a preview's rows reach its node, beside the row.
    const REACHED: &'static str = "preview_reached";
    /// The entries holding each episode's video of one camera, numbered.
    const VIDEOS: &'static str = "camera_video";
    /// The entries holding each episode's instruction on one topic, numbered.
    const INSTRUCTIONS: &'static str = "instruction";
    /// What the answers table is read as.
    const STORED: &'static str = "a";

    pub(in crate::activities::curation) fn new(
        dataset: &DatasetSelection,
        source: PathSource,
    ) -> Self {
        Self {
            dataset: dataset.clone(),
            source,
            cameras: Vec::new(),
            instructions: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// Which reading of the path the row is built for.
    pub(in crate::activities::curation) fn source(&self) -> PathSource {
        self.source
    }
    /// The episode's packet video of `camera`.
    pub(in crate::activities::curation) fn video(&mut self, camera: &str) -> String {
        let entry = Self::entry(Self::VIDEOS, Self::read_once(&mut self.cameras, camera));
        format!("{entry}.source")
    }
    /// The first message the episode records on `topic`: its task.
    pub(in crate::activities::curation) fn instruction(&mut self, topic: &str) -> String {
        let entry = Self::entry(
            Self::INSTRUCTIONS,
            Self::read_once(&mut self.instructions, topic),
        );
        format!("{entry}.task")
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

    /// The row's entries over the rows that reach it, and the relation
    /// handing those rows on with every answer beside them; nothing when no
    /// node asks anything. Published, the rows are whole episodes, one each,
    /// so the row is built on them and each one's start is its first message.
    /// Previewed, they are the parent's preview rows, and the row is the
    /// bound episode's alone, read from its listing and joined to each.
    pub(super) fn over(self, reaching: &Relation) -> Result<Option<StepSql>, PathError> {
        if self.answers.is_empty() {
            return Ok(None);
        }
        let (episodes, origin) = (Self::EPISODES, Self::ORIGIN);
        let carried = RowAnswers(
            self.answers
                .iter()
                .map(|(column, _)| column.to_string())
                .collect(),
        );
        let mut entries = self.reads(Some(reaching));
        let relation = match self.source {
            PathSource::Published => {
                entries.push(format!(
                    "{episodes} AS (SELECT {}, i.start_ns AS {origin}{} FROM {} i{})",
                    reaching.columns_of("i."),
                    self.answer_columns(),
                    reaching.name(),
                    self.joins("i.episode_id")?
                ));
                reaching.renamed(episodes)
            }
            PathSource::Preview | PathSource::Fill => {
                entries.extend([
                    self.bound_episode()?,
                    format!(
                        "{} AS (SELECT {}{} FROM {} i JOIN {episodes} e ON e.episode_id = i.episode_id)",
                        Self::REACHED,
                        reaching.columns_of("i."),
                        carried.following("e."),
                        reaching.name()
                    ),
                ]);
                reaching.renamed(Self::REACHED)
            }
        };
        Ok(Some(StepSql::new(entries, relation.carrying(carried))))
    }

    /// The one statement filling the one answer the row asks for, for the
    /// episode bound as `$1`: `episode_id` and its `answer`, or no row for an
    /// episode without what the call reads.
    pub(in crate::activities::curation) fn answering(self) -> Result<String, PathError> {
        let [(column, _)] = self.answers.as_slice() else {
            return Err(PathError::new("A fill asks each episode one thing."));
        };
        let mut entries = self.reads(None);
        entries.push(self.bound_episode()?);
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

    /// The bound episode's row, read from the dataset's listing.
    fn bound_episode(&self) -> Result<String, PathError> {
        Ok(format!(
            "{} AS (SELECT e.id AS episode_id, CAST(e.message_start_time AS BIGINT) AS {}{} \
             FROM spql.mcap.episodes({}) e{} WHERE e.id = $1)",
            Self::EPISODES,
            Self::ORIGIN,
            self.answer_columns(),
            SqlLiteral::text(&self.dataset.sql_name()),
            self.joins("e.id")?
        ))
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
    /// with: of the episodes that reach it, or of the episode bound as `$1`.
    fn reads(&self, reaching: Option<&Relation>) -> Vec<String> {
        let dataset = self.dataset.sql_name();
        let members = match (self.source, reaching) {
            (PathSource::Published, Some(reaching)) => {
                format!("r.id IN (SELECT episode_id FROM {})", reaching.name())
            }
            _ => "r.id = $1".to_owned(),
        };
        let videos = self.cameras.iter().enumerate().map(|(index, camera)| {
            let videos = Self::entry(Self::VIDEOS, index);
            CameraVideo::new(&dataset, camera)
                .videos((&format!("{videos}_packets"), &videos), &members)
        });
        let instructions = self.instructions.iter().enumerate().map(|(index, topic)| {
            format!(
                "{} AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, \
                 ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time) AS position \
                 FROM spql.mcap.read_topic({}, {}) r WHERE {members}) instructions WHERE position = 1)",
                Self::entry(Self::INSTRUCTIONS, index),
                SqlLiteral::text(&dataset),
                SqlLiteral::text(topic)
            )
        });
        videos.chain(instructions).collect()
    }

    /// The answers table joined to the episode `id` names when an answer is
    /// stored, every video the row reads, and every instruction beside it:
    /// an episode without the camera has no answer, and one without an
    /// instruction is asked without a task.
    fn joins(&self, id: &str) -> Result<String, PathError> {
        let stored = self
            .answers
            .iter()
            .any(|(_, answer)| matches!(answer, RowAnswer::Stored));
        let answers = if stored {
            let table = AnswerTable::of(&self.dataset).map_err(PathError::new)?;
            let alias = Self::STORED;
            format!(
                " JOIN {} {alias} ON {alias}.episode_id = {id}",
                table.quoted()
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
        Ok(std::iter::once(answers)
            .chain(videos)
            .chain(instructions)
            .collect())
    }

    fn entry(kind: &str, index: usize) -> String {
        format!("{kind}_{}", index + 1)
    }

    /// Where `read` is among `reads`, added the first time it is asked for.
    fn read_once(reads: &mut Vec<String>, read: &str) -> usize {
        match reads.iter().position(|existing| existing == read) {
            Some(index) => index,
            None => {
                reads.push(read.to_owned());
                reads.len() - 1
            }
        }
    }
}

/// The answers of episode rows riding on interval rows, each until the step
/// of the node that asked for it unnests it, and with them the episode's
/// first message, which every answer's times count from. Each rides under
/// the name of the column that stores it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct RowAnswers(Vec<String>);

impl RowAnswers {
    /// The columns, each after `qualifier` (such as `r.`) and led by a comma,
    /// to follow others in a `SELECT`; nothing when no answer rides.
    pub(in crate::activities::curation) fn following(&self, qualifier: &str) -> String {
        self.columns()
            .map(|column| format!(", {qualifier}{column}"))
            .collect()
    }
    pub(super) fn columns(&self) -> impl Iterator<Item = &str> + '_ {
        let origin = (!self.0.is_empty()).then_some(EpisodeRow::ORIGIN);
        origin.into_iter().chain(self.0.iter().map(String::as_str))
    }
    /// What rides on past the step that unnests `answer`, which must be
    /// riding here.
    pub(in crate::activities::curation) fn without(
        &self,
        answer: &AnswerColumn,
    ) -> Result<Self, PathError> {
        if !self.0.iter().any(|riding| riding == answer.as_str()) {
            return Err(PathError::new(format!(
                "Nothing that reaches this node carries each episode's {answer}."
            )));
        }
        Ok(Self(
            self.0
                .iter()
                .filter(|riding| *riding != answer.as_str())
                .cloned()
                .collect(),
        ))
    }
}
