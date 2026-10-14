//! Each episode's row: the episode's first message, which every stored time
//! counts from, and what the nodes of a path stored about the whole episode,
//! one column each, which their entries read.
use super::PathSource;
use crate::activities::curation::graph::DatasetStep;
use spiral_robotics_common::curation::{AnswerColumn, AnswerTable};
use spiral_robotics_common::datasets::DatasetSelection;

/// Each episode's row: every answer the nodes of a path stored about the
/// whole episode, read from the dataset's answers table, one column each,
/// named for the column that stores it. Published, the row is built on the
/// dataset's listing, one per episode; previewed, it is the listed
/// episodes'.
pub(in crate::activities::curation) struct EpisodeRow {
    dataset: DatasetSelection,
    source: PathSource,
    answers: Vec<AnswerColumn>,
}

impl EpisodeRow {
    /// The column the episode's first message rides in, in log time.
    pub(in crate::activities::curation) const ORIGIN: &'static str = "origin_ns";
    /// The entry holding each episode's row.
    pub(in crate::activities::curation) const EPISODES: &'static str = "episodes";
    /// What the answers table is read as.
    const STORED: &'static str = "a";
    /// What the dataset's listing of the previewed episodes is read as.
    const LISTED: &'static str = "e";

    /// The row `source`, a published view or a preview, reads.
    pub(in crate::activities::curation) fn new(
        dataset: &DatasetSelection,
        source: PathSource,
    ) -> Self {
        Self {
            dataset: dataset.clone(),
            source,
            answers: Vec::new(),
        }
    }

    /// Read each episode's answers in `column` from the answers table; two
    /// nodes making one call on a path read its one stored column.
    pub(in crate::activities::curation) fn stored(&mut self, column: &AnswerColumn) {
        if !self.answers.contains(column) {
            self.answers.push(column.clone());
        }
    }

    /// The row's entry, [`Self::EPISODES`]; nothing when no node stored
    /// anything. Published, each episode's first message is the start of
    /// its listed span.
    pub(in crate::activities::curation) fn entries(self) -> Vec<String> {
        if self.answers.is_empty() {
            return Vec::new();
        }
        vec![match self.source {
            PathSource::Published => format!(
                "{} AS (SELECT d.episode_id, d.{origin}{} FROM {} d{})",
                Self::EPISODES,
                self.answer_columns(),
                DatasetStep::NAME,
                self.joins("d.episode_id"),
                origin = Self::ORIGIN,
            ),
            PathSource::Preview { episodes } => self.listed_episodes(episodes),
        }]
    }

    /// The rows of the `episodes` episodes a preview lists, `$1` through
    /// `$N`, read from the dataset's listing.
    fn listed_episodes(&self, episodes: usize) -> String {
        let listed = Self::LISTED;
        let bound = (1..=episodes)
            .map(|place| format!("${place}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} AS (SELECT {listed}.id AS episode_id, CAST({listed}.message_start_time AS BIGINT) AS {}{} \
             FROM {} {listed}{} WHERE {listed}.id IN ({bound}))",
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
            .map(|column| format!(", {}.{column}.value AS {column}", Self::STORED))
            .collect()
    }

    /// The answers table joined to the episode `id` names.
    fn joins(&self, id: &str) -> String {
        let alias = Self::STORED;
        format!(
            " JOIN {} {alias} ON {alias}.episode_id = {id}",
            AnswerTable::of(&self.dataset).table()
        )
    }
}
