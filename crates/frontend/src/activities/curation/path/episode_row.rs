//! Each episode's row: what the nodes of a path ask a model about the whole
//! episode, built once where the path still holds one row per episode, and
//! the answers that then ride on the interval rows below it until the step
//! that unnests each.
use super::{PathError, PathSource, Relation, StepSql};
use spiral_robotics_common::datasets::{CameraVideo, DatasetSelection};
use spiral_robotics_common::sql::SqlLiteral;

/// Each episode's row, where a path still holds one row per episode: every
/// answer the nodes ask a model about the whole episode, one column each,
/// over the camera videos and the instructions the calls read. A camera two
/// models watch is read once, and no video leaves the row. Every answer
/// counts from the episode's first message, which the row carries beside
/// them.
pub(in crate::activities::curation) struct EpisodeRow {
    /// The dataset's SQL name.
    dataset: String,
    source: PathSource,
    cameras: Vec<String>,
    instructions: Vec<String>,
    asked: Vec<(&'static str, String)>,
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

    pub(super) fn new(dataset: &DatasetSelection, source: PathSource) -> Self {
        Self {
            dataset: dataset.sql_name(),
            source,
            cameras: Vec::new(),
            instructions: Vec::new(),
            asked: Vec::new(),
        }
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
    /// The trailing argument of a model call on this row.
    pub(in crate::activities::curation) fn on_error(&self) -> &'static str {
        self.source.on_error()
    }
    /// Ask each episode for `column`, answered by `call`.
    pub(in crate::activities::curation) fn ask(
        &mut self,
        column: &'static str,
        call: String,
    ) -> Result<(), PathError> {
        if self.asked.iter().any(|(asked, _)| *asked == column) {
            return Err(PathError::new(format!(
                "Two nodes of this path ask each episode for its {column}."
            )));
        }
        self.asked.push((column, call));
        Ok(())
    }

    /// The row's entries over the rows that reach it, and the relation
    /// handing those rows on with every answer beside them; nothing when no
    /// node asks anything. Published, the rows are whole episodes, one each,
    /// so the row is built on them and each one's start is its first message.
    /// Previewed, they are the parent's preview rows, and the row is the
    /// bound episode's alone, read from its listing and joined to each.
    pub(super) fn over(self, reaching: &Relation) -> Option<StepSql> {
        if self.asked.is_empty() {
            return None;
        }
        let (episodes, origin) = (Self::EPISODES, Self::ORIGIN);
        let listed = SqlLiteral::text(&self.dataset);
        let calls = self
            .asked
            .iter()
            .map(|(column, call)| format!(", {call} AS {column}"))
            .collect::<String>();
        let carried = RowAnswers(self.asked.iter().map(|(column, _)| *column).collect());
        let mut entries = self.reads(reaching);
        let relation = match self.source {
            PathSource::Published => {
                entries.push(format!(
                    "{episodes} AS (SELECT {}, i.start_ns AS {origin}{calls} FROM {} i{})",
                    reaching.columns_of("i."),
                    reaching.name(),
                    self.joins("i.episode_id")
                ));
                reaching.renamed(episodes)
            }
            PathSource::Preview => {
                entries.extend([
                    format!(
                        "{episodes} AS (SELECT e.id AS episode_id, CAST(e.message_start_time AS BIGINT) AS {origin}{calls} \
                         FROM spql.mcap.episodes({listed}) e{} WHERE e.id = $1)",
                        self.joins("e.id")
                    ),
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
        Some(StepSql::new(entries, relation.carrying(carried)))
    }

    /// The entries reading every camera video and instruction the row asks
    /// with, of the episodes that reach it.
    fn reads(&self, reaching: &Relation) -> Vec<String> {
        let dataset = &self.dataset;
        let members = match self.source {
            PathSource::Published => {
                format!("r.id IN (SELECT episode_id FROM {})", reaching.name())
            }
            PathSource::Preview => "r.id = $1".to_owned(),
        };
        let videos = self.cameras.iter().enumerate().map(|(index, camera)| {
            let videos = Self::entry(Self::VIDEOS, index);
            CameraVideo::new(dataset, camera)
                .videos((&format!("{videos}_packets"), &videos), &members)
        });
        let instructions = self.instructions.iter().enumerate().map(|(index, topic)| {
            format!(
                "{} AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, \
                 ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time) AS position \
                 FROM spql.mcap.read_topic({}, {}) r WHERE {members}) instructions WHERE position = 1)",
                Self::entry(Self::INSTRUCTIONS, index),
                SqlLiteral::text(dataset),
                SqlLiteral::text(topic)
            )
        });
        videos.chain(instructions).collect()
    }

    /// Every video the row reads joined to the episode `id` names, and every
    /// instruction beside it: an episode without the camera has no answer,
    /// and one without an instruction is asked without a task.
    fn joins(&self, id: &str) -> String {
        let videos = (0..self.cameras.len()).map(|index| {
            let entry = Self::entry(Self::VIDEOS, index);
            format!(" JOIN {entry} ON {entry}.id = {id}")
        });
        let instructions = (0..self.instructions.len()).map(|index| {
            let entry = Self::entry(Self::INSTRUCTIONS, index);
            format!(" LEFT JOIN {entry} ON {entry}.id = {id}")
        });
        videos.chain(instructions).collect()
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
/// first message, which every answer's times count from.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct RowAnswers(Vec<&'static str>);

impl RowAnswers {
    /// The columns, each after `qualifier` (such as `r.`) and led by a comma,
    /// to follow others in a `SELECT`; nothing when no answer rides.
    pub(in crate::activities::curation) fn following(&self, qualifier: &str) -> String {
        self.columns()
            .map(|column| format!(", {qualifier}{column}"))
            .collect()
    }
    pub(super) fn columns(&self) -> impl Iterator<Item = &'static str> + '_ {
        let origin = (!self.0.is_empty()).then_some(EpisodeRow::ORIGIN);
        origin.into_iter().chain(self.0.iter().copied())
    }
    /// What rides on past the step that unnests `answer`, which must be
    /// riding here.
    pub(in crate::activities::curation) fn without(&self, answer: &str) -> Result<Self, PathError> {
        if !self.0.contains(&answer) {
            return Err(PathError::new(format!(
                "Nothing that reaches this node carries each episode's {answer}."
            )));
        }
        Ok(Self(
            self.0
                .iter()
                .copied()
                .filter(|riding| *riding != answer)
                .collect(),
        ))
    }
}
