//! What a Break down subtasks node adds to a path.
//!
//! The node asks each episode's row for `spql.ai.breakdown_task` over the
//! chosen camera's video, with the episode's recorded instruction as its task
//! when the dataset records one. The model reads each episode whole, so what
//! it is asked never depends on anything above the node. Its step unnests
//! the subtasks from every row that reaches the node and cuts them to that
//! row's interval. The function answers on the video's presentation
//! timeline, which Studio's packets count from the episode's first message,
//! so a subtask's log time is that message's time plus its own.
use super::subtask::SubtaskRow;
use crate::activities::curation::document::{DEFAULT_ANNOTATION_MODEL, SubtaskAnnotation};
use crate::activities::curation::path::{
    EpisodeRow, PathError, PathFragment, PathStep, Relation, StepSql,
};
use spiral_robotics_common::sql::SqlLiteral;

/// The column each episode's subtasks ride in until the node's step unnests
/// them.
const SUBTASKS: &str = "subtasks";

impl PathFragment for SubtaskAnnotation {
    fn episode_columns(&self, row: &mut EpisodeRow) -> Result<(), PathError> {
        let camera = self
            .camera()
            .ok_or_else(|| PathError::new("Choose the camera Break down reads."))?;
        let interval = SqlLiteral::number(self.sample_interval_seconds())
            .ok_or_else(|| PathError::new("The sample interval must be a number."))?;
        let video = row.video(camera);
        let task = self
            .task_topic()
            .map(|topic| format!(", task => {}", row.instruction(topic)))
            .unwrap_or_default();
        let call = format!(
            "spql.ai.breakdown_task({video}, sample_interval => {interval}, model => {}{task}{})",
            SqlLiteral::text(DEFAULT_ANNOTATION_MODEL),
            row.on_error()
        );
        row.ask(SUBTASKS, call)
    }

    /// Every subtask the model found in the episode of every row that
    /// reaches the node, cut to that row's interval.
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let rides = input.answers().without(SUBTASKS)?;
        let (name, subtask, origin) = (step.name(), SubtaskRow::DESCRIPTION, EpisodeRow::ORIGIN);
        let entries = vec![
            format!(
                "{name}_subtasks AS (SELECT r.episode_id, r.start_ns AS reached_start_ns, r.end_ns AS reached_end_ns{}, \
                 r.{origin} + CAST(s.start_ns AS BIGINT) AS start_ns, r.{origin} + CAST(s.end_ns AS BIGINT) AS end_ns, \
                 s.description AS {subtask} FROM {} r CROSS JOIN LATERAL UNNEST(r.{SUBTASKS}) AS s)",
                rides.following("r."),
                input.name()
            ),
            format!(
                "{name} AS (SELECT episode_id, GREATEST(start_ns, reached_start_ns) AS start_ns, \
                 LEAST(end_ns, reached_end_ns) AS end_ns, {subtask}{} FROM {name}_subtasks \
                 WHERE start_ns < reached_end_ns AND end_ns > reached_start_ns)",
                rides.following("")
            ),
        ];
        Ok(StepSql::new(
            entries,
            Relation::new(name, vec![subtask])
                .publishing(subtask)
                .carrying(rides),
        ))
    }
}
