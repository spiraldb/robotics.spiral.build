//! The step a Break down subtasks node adds to a path.
//!
//! The node compiles to `spql.ai.breakdown_task` over each episode's video of
//! the chosen camera, with the episode's recorded instruction as its task when
//! the dataset records one. The model reads each episode whole, so what it is
//! asked never depends on anything above the node; its subtasks are then cut
//! to the intervals that reach the node. The function answers on the video's
//! presentation timeline, which Studio's packets count from the episode's
//! first message, so a subtask's log time is that message's time plus its own.
use super::subtask::SubtaskRow;
use crate::activities::curation::document::SubtaskAnnotation;
use crate::activities::curation::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use spiral_robotics_common::datasets::CameraVideo;
use spiral_robotics_common::sql::SqlLiteral;

/// Every subtask the model found in every episode that reaches the node, cut
/// to the intervals it reached the node in.
impl PathFragment for SubtaskAnnotation {
    fn step_sql(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let camera = self
            .camera()
            .ok_or_else(|| PathError::new("Choose the camera Break down reads."))?;
        let interval = SqlLiteral::number(self.sample_interval_seconds())
            .ok_or_else(|| PathError::new("The sample interval must be a number."))?;
        let (name, from) = (step.name(), input.name());
        let dataset = step.dataset().sql_name();
        let listed = SqlLiteral::text(&dataset);
        // Only the episodes that reach the node are read, and a preview reads
        // only its own.
        let members = std::iter::once(format!("r.id IN (SELECT episode_id FROM {from})"))
            .chain(step.source().members("r.id"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut entries = vec![CameraVideo::new(&dataset, camera).videos(
            (&format!("{name}_packets"), &format!("{name}_videos")),
            &members,
        )];
        // The first instruction an episode records is its task. The videos
        // are joined to it before the call: Spiral joins scalar columns, not
        // the list of structs the call answers with.
        let (task, tasks) = match self.task_topic() {
            Some(topic) => {
                entries.push(format!(
                    "{name}_tasks AS (SELECT id, task FROM (SELECT r.id, r.value.data AS task, \
                     ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.log_time) AS position \
                     FROM spql.mcap.read_topic({listed}, {}) r WHERE {members}) instructions WHERE position = 1)",
                    SqlLiteral::text(topic)
                ));
                (
                    ", task => t.task",
                    format!(" LEFT JOIN {name}_tasks t ON t.id = v.id"),
                )
            }
            None => ("", String::new()),
        };
        entries.extend([
            format!(
                "{name}_breakdowns AS (SELECT v.id AS episode_id, spql.ai.breakdown_task(v.source, \
                 sample_interval => {interval}, model => {}{task}{}) AS subtasks FROM {name}_videos v{tasks})",
                SqlLiteral::text(self.model()),
                step.source().on_error()
            ),
            format!(
                "{name}_offsets AS (SELECT b.episode_id, CAST(s.start_ns AS BIGINT) AS start_offset_ns, \
                 CAST(s.end_ns AS BIGINT) AS end_offset_ns, s.description AS {subtask} \
                 FROM {name}_breakdowns b CROSS JOIN LATERAL UNNEST(b.subtasks) AS s)",
                subtask = SubtaskRow::DESCRIPTION
            ),
            format!(
                "{name}_subtasks AS (SELECT o.episode_id, e.origin_ns + o.start_offset_ns AS start_ns, \
                 e.origin_ns + o.end_offset_ns AS end_ns, o.{subtask} FROM {name}_offsets o \
                 JOIN (SELECT id, CAST(message_start_time AS BIGINT) AS origin_ns FROM spql.mcap.episodes({listed})) e \
                 ON e.id = o.episode_id)",
                subtask = SubtaskRow::DESCRIPTION
            ),
            format!(
                "{name} AS (SELECT s.episode_id, GREATEST(s.start_ns, i.start_ns) AS start_ns, \
                 LEAST(s.end_ns, i.end_ns) AS end_ns, s.{subtask} FROM {name}_subtasks s JOIN {from} i \
                 ON i.episode_id = s.episode_id AND s.start_ns < i.end_ns AND s.end_ns > i.start_ns)",
                subtask = SubtaskRow::DESCRIPTION
            ),
        ]);
        Ok(StepSql::new(
            entries,
            Relation::new(name, vec![SubtaskRow::DESCRIPTION]).publishing(SubtaskRow::DESCRIPTION),
        ))
    }
}
