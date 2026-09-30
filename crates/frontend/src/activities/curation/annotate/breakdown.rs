//! What a Break down subtasks node adds to a path.
//!
//! The node asks each episode for `spql.ai.breakdown_task` over the chosen
//! camera's video, with the episode's recorded instruction as its task when
//! the dataset records one. A fill stores the answer in the dataset's
//! answers table, and the published view and the preview read it from there.
//! The model reads each episode whole, so what it is asked never
//! depends on anything above the node. Its step lists the subtasks of every
//! row that reaches the node, each with its place in the answer, which the
//! stored judgments below read by, and cuts them to that row's interval. The
//! function answers on the video's presentation timeline, which Studio's
//! packets count from the episode's first message, so a subtask's log time
//! is that message's time plus its own.
use super::subtask::SubtaskRow;
use crate::activities::curation::document::{DEFAULT_ANNOTATION_MODEL, SubtaskAnnotation};
use crate::activities::curation::path::{
    EpisodeRow, PathError, PathFragment, PathSource, PathStep, Relation, StepSql, StoredAnswers,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::sql::SqlLiteral;

impl SubtaskAnnotation {
    /// The camera the model watches.
    fn watched(&self) -> Result<&str, PathError> {
        self.camera()
            .ok_or_else(|| PathError::new("Choose the camera Break down reads."))
    }
    /// The model's call over the episode's row.
    fn call(&self, row: &mut EpisodeRow) -> Result<String, PathError> {
        let interval = SqlLiteral::number(self.sample_interval_seconds())
            .ok_or_else(|| PathError::new("The sample interval must be a number."))?;
        let video = row.video(self.watched()?);
        let task = self
            .task_topic()
            .map(|topic| format!(", task => {}", row.instruction(topic)))
            .unwrap_or_default();
        Ok(format!(
            "spql.ai.breakdown_task({video}, sample_interval => {interval}, model => {}{task})",
            SqlLiteral::text(DEFAULT_ANNOTATION_MODEL)
        ))
    }
}

/// Each episode's subtasks are stored under the model, the camera, the
/// sample interval, and the instruction topic it was told its task by.
impl StoredAnswers for SubtaskAnnotation {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let call = serde_json::json!([
            "spql.ai.breakdown_task",
            DEFAULT_ANNOTATION_MODEL,
            self.watched()?,
            self.sample_interval_seconds(),
            self.task_topic(),
        ]);
        AnswerColumn::of("breakdown_task", &call).map_err(PathError::new)
    }

    fn answer_source(
        &self,
        dataset: &DatasetSelection,
        (column, _): (&AnswerColumn, Option<&AnswerColumn>),
    ) -> Result<AnswerSource, PathError> {
        let mut row = EpisodeRow::new(dataset, PathSource::Fill);
        self.episode_columns(&mut row, column)?;
        Ok(AnswerSource::Statement {
            statement: row.answering()?,
        })
    }
}

impl PathFragment for SubtaskAnnotation {
    fn episode_columns(
        &self,
        row: &mut EpisodeRow,
        column: &AnswerColumn,
    ) -> Result<(), PathError> {
        match row.source() {
            PathSource::Published | PathSource::Preview => row.stored(column),
            PathSource::Fill => {
                let call = self.call(row)?;
                row.asked(column, call)
            }
        }
    }

    /// Every subtask the model found in the episode of every row that
    /// reaches the node, at its place in the answer, cut to that row's
    /// interval.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let answer = step.column()?;
        let rides = input.answers().without(answer)?;
        let (name, subtask, index, origin) = (
            step.name(),
            SubtaskRow::DESCRIPTION,
            SubtaskRow::INDEX,
            EpisodeRow::ORIGIN,
        );
        let entries = vec![
            format!(
                "{name}_listed AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns{}, \
                 r.{origin} AS listed_origin_ns, g._ AS {index}, spql.list.get(r.{answer}, g._) AS listed \
                 FROM {} r CROSS JOIN LATERAL spql.range(CAST(spql.list.len(r.{answer}) AS BIGINT)) g)",
                rides.following("r."),
                input.name()
            ),
            format!(
                "{name}_subtasks AS (SELECT l.episode_id, l.reached_start_ns, l.reached_end_ns{}, \
                 l.listed_origin_ns + CAST(l.listed.start_ns AS BIGINT) AS start_ns, l.listed_origin_ns + CAST(l.listed.end_ns AS BIGINT) AS end_ns, \
                 l.listed.description AS {subtask}, l.{index} FROM {name}_listed l)",
                rides.following("l.")
            ),
            format!(
                "{name} AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, \
                 LEAST(end_ns, reached_end_ns) AS end_ns, {subtask}, {index}{} FROM {name}_subtasks \
                 WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns)",
                rides.following("")
            ),
        ];
        Ok(StepSql::new(
            entries,
            Relation::new(name, vec![subtask, index])
                .publishing(subtask)
                .carrying(rides),
        ))
    }
}
