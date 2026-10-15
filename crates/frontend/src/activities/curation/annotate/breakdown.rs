//! What a Break down subtasks node adds to a path, and the listing of its
//! subtasks every judgment of them reads too.
//!
//! The node asks the backend to break each episode down over the chosen
//! camera's video, with the episode's task when the dataset records one: its
//! recorded instruction, or the task a LeRobot dataset lists. A fill stores
//! the answer in the dataset's answers table, and the published view and the
//! preview read it from there. The model reads each episode whole, so what
//! it is asked never depends on anything above the node. Its entries list
//! the subtasks of every episode, each with its place in the answer, which
//! the stored judgments below read by. The answer is on the video's
//! presentation timeline, which Studio's packets count from the episode's
//! first message, so a subtask's log time is that message's time plus its
//! own.
use crate::activities::curation::document::{DEFAULT_ANNOTATION_MODEL, SubtaskAnnotation};
use crate::activities::curation::path::{
    ColumnMeaning, EpisodeRow, ModelEntries, ModelNode, ModelStep, PathError, TableColumn,
};
use spiral_robotics_common::curation::{
    AnswerColumn, AnswerSource, SampleInterval, SubtaskBreakdown,
};
use spiral_robotics_common::datasets::EpisodeTask;

impl SubtaskAnnotation {
    /// The camera the model watches.
    fn watched(&self) -> Result<&str, PathError> {
        self.camera()
            .ok_or_else(|| PathError::new("Choose the camera Break down reads."))
    }
}

/// Each episode's subtasks are stored under the model, the camera, the
/// sample interval, and where it was told its task from: the instruction
/// topic, or the task its dataset lists. The call keeps the name of the
/// Spiral function that first answered it, so what that stored is read
/// without asking again.
impl ModelNode for SubtaskAnnotation {
    fn answer_column(&self, _subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let task = match self.task() {
            Some(EpisodeTask::Recorded(topic)) => serde_json::json!(topic),
            Some(EpisodeTask::Listed) => serde_json::json!({"listed": "tasks"}),
            None => serde_json::Value::Null,
        };
        let call = serde_json::json!([
            DEFAULT_ANNOTATION_MODEL,
            self.watched()?,
            self.sample_interval_seconds(),
            task,
        ]);
        AnswerColumn::of("break_down", &call).map_err(PathError::new)
    }

    fn answer_source(&self, _step: &ModelStep<'_>) -> Result<AnswerSource, PathError> {
        let interval =
            SampleInterval::from_seconds(self.sample_interval_seconds()).map_err(PathError::new)?;
        Ok(AnswerSource::BreakDown(SubtaskBreakdown::new(
            self.watched()?,
            interval,
            DEFAULT_ANNOTATION_MODEL,
            self.task(),
        )))
    }

    /// Every subtask the model found in the episode, at its place in the
    /// answer, in log time.
    fn entries(&self, step: &ModelStep<'_>) -> Result<ModelEntries, PathError> {
        Ok(SubtaskListing::new(step, step.column()).entries(|entries| {
            entries.folding(
                TableColumn::of_node(step.node(), &step.name(), ColumnMeaning::Subtasks),
                (
                    "STRUCT_PACK(description := description, start_ns := start_ns, end_ns := end_ns)"
                        .to_owned(),
                    None,
                ),
            )
        }))
    }
}

/// The subtasks a Break down stored for each episode, listed one per entry
/// with their place in its answer and their log times, and beside each the
/// fields of the judgment a node stored for it at the same place, when it
/// judges them.
pub(in crate::activities::curation) struct SubtaskListing<'a> {
    step: &'a ModelStep<'a>,
    subtasks: &'a AnswerColumn,
    judged: &'a [(&'a str, &'a str)],
}

impl<'a> SubtaskListing<'a> {
    /// What a preview reads of every listing's entries, before any judgment.
    const LISTED: [&'static str; 4] = ["episode_id", "description", "start_ns", "end_ns"];

    /// The subtasks the Break down column `subtasks` stores.
    pub(in crate::activities::curation) fn new(
        step: &'a ModelStep<'a>,
        subtasks: &'a AnswerColumn,
    ) -> Self {
        Self {
            step,
            subtasks,
            judged: &[],
        }
    }

    /// The same, with `judged` read of the node's own stored judgment of
    /// each: every field as its expression over `judgment` and its name.
    pub(in crate::activities::curation) fn judged_as(
        self,
        judged: &'a [(&'a str, &'a str)],
    ) -> Self {
        Self { judged, ..self }
    }

    /// The node's entries, folded into its columns by `fold`. A preview reads
    /// every field of each entry.
    pub(in crate::activities::curation) fn entries(
        &self,
        fold: impl FnOnce(ModelEntries) -> ModelEntries,
    ) -> ModelEntries {
        let (step, subtasks, origin) = (self.step, self.subtasks, EpisodeRow::ORIGIN);
        let listed = format!("{}_listed", step.name());
        let judgment = if self.judged.is_empty() {
            String::new()
        } else {
            format!(", spql.list.get(r.{}, g._) AS judgment", step.column())
        };
        let judged = self
            .judged
            .iter()
            .map(|(field, name)| format!(", l.{field} AS {name}"))
            .collect::<String>();
        let entries = vec![
            format!(
                "{listed} AS (SELECT r.episode_id, r.{origin}, g._ AS subtask_index, \
                 spql.list.get(r.{subtasks}, g._) AS listed{judgment} FROM {} r \
                 CROSS JOIN LATERAL spql.range(CAST(spql.list.len(r.{subtasks}) AS BIGINT)) g)",
                EpisodeRow::EPISODES,
            ),
            format!(
                "{} AS (SELECT l.episode_id, l.subtask_index, l.listed.description AS description, \
                 l.{origin} + CAST(l.listed.start_ns AS BIGINT) AS start_ns, \
                 l.{origin} + CAST(l.listed.end_ns AS BIGINT) AS end_ns{judged} FROM {listed} l)",
                step.entries(),
            ),
        ];
        let previewed = Self::LISTED
            .into_iter()
            .chain(self.judged.iter().map(|(_, name)| *name))
            .collect::<Vec<_>>()
            .join(", ");
        fold(ModelEntries::new(entries, "subtask_index", previewed))
    }
}
