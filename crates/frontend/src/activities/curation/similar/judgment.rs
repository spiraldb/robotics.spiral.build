//! The steps Find similar and Group similar subtasks add to a path: one text
//! judgment of every subtask's description that reaches the node.
//!
//! Find similar asks `spql.ai.similarity` against its query, and Group
//! similar `spql.ai.classify` over its categories, written into the
//! statement as a literal list. A fill judges each subtask the Break down
//! above stored for an episode and collects the answers into one list
//! aligned with them; the published view and the preview read each
//! subtask's judgment at its place in that list. The node's own rule keeps
//! part of them.
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{SubtaskGrouping, SubtaskSearch};
use crate::activities::curation::path::{
    EpisodeRow, PathError, PathFragment, PathStep, StepSql, StoredAnswers,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource, AnswerTable};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::sql::SqlLiteral;

/// The decision model both judgments are asked of, pinned so every published
/// answer names the model that gave it.
const JUDGMENT_MODEL: &str = "typesafe/jev-1.13";

/// A text judgment of every subtask in a list, as either node asks it.
trait SubtaskJudgment {
    /// What the node measures, which names its column on the path.
    const MEASURE: &'static str;
    /// The function its call is to, which names its stored column.
    const FUNCTION: &'static str;
    /// Everything besides the subtasks that changes the answer, or why the
    /// node cannot ask yet.
    fn arguments(&self) -> Result<serde_json::Value, PathError>;
    /// The call judging one subtask's `description`.
    fn call(&self, description: &str) -> Result<String, PathError>;

    /// The column the judgments are stored in: its function, its model, what
    /// it asks, and the Break down column whose subtasks it judges.
    fn column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let call = serde_json::json!([
            Self::FUNCTION,
            JUDGMENT_MODEL,
            self.arguments()?,
            Self::judged(subtasks)?.as_str()
        ]);
        AnswerColumn::of(Self::MEASURE, &call).map_err(PathError::new)
    }

    /// The Break down column a judgment reads, which one above it must store.
    fn judged(subtasks: Option<&AnswerColumn>) -> Result<&AnswerColumn, PathError> {
        subtasks.ok_or_else(|| {
            PathError::new("This node reads the subtasks a Break down above it describes.")
        })
    }
}

/// Every subtask that reaches the node, with its judgment read from the
/// stored list at the subtask's place; the list rides on for any node below
/// asking the same.
impl<J: SubtaskJudgment> PathFragment for J {
    fn episode_columns(
        &self,
        row: &mut EpisodeRow,
        column: &AnswerColumn,
    ) -> Result<(), PathError> {
        row.stored(column)
    }

    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let stored = step.column()?;
        input.answers().without(stored)?;
        let index = input.measured(SubtaskRow::INDEX).ok_or_else(|| {
            PathError::new("This node reads the subtasks a Break down above it lists.")
        })?;
        let relation = input.measuring(step.name(), Self::MEASURE);
        let column = relation
            .measured(Self::MEASURE)
            .expect("the relation carries what it just measured");
        let entry = format!(
            "{} AS (SELECT {}, spql.list.get({stored}, {index}) AS {column} FROM {})",
            step.name(),
            input.columns(),
            input.name()
        );
        Ok(StepSql::new(vec![entry], relation))
    }
}

/// A fill judges, for the episode bound as `$1`, each subtask the Break
/// down above stored, collected in their order.
impl<J: SubtaskJudgment> StoredAnswers for J {
    fn answer_column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        self.column(subtasks)
    }

    fn answer_source(
        &self,
        dataset: &DatasetSelection,
        (_, subtasks): (&AnswerColumn, Option<&AnswerColumn>),
    ) -> Result<AnswerSource, PathError> {
        let table = AnswerTable::of(dataset).map_err(PathError::new)?;
        let call = self.call("s.listed.description")?;
        Ok(AnswerSource::Statement {
            statement: format!(
                "WITH e AS (SELECT a.episode_id, a.{}.value AS subtasks FROM {} a WHERE a.episode_id = $1), \
                 s AS (SELECT e.episode_id, spql.list.get(e.subtasks, g._) AS listed \
                 FROM e CROSS JOIN LATERAL spql.range(CAST(spql.list.len(e.subtasks) AS BIGINT)) g) \
                 SELECT s.episode_id, spql.list.collect({call}) AS answer FROM s GROUP BY s.episode_id",
                J::judged(subtasks)?,
                table.quoted()
            ),
        })
    }
}

/// How likely each subtask is the operation the query names.
impl SubtaskJudgment for SubtaskSearch {
    const MEASURE: &'static str = SubtaskRow::SIMILARITY;
    const FUNCTION: &'static str = "spql.ai.similarity";

    fn arguments(&self) -> Result<serde_json::Value, PathError> {
        let query = self.query().trim();
        if query.is_empty() {
            return Err(PathError::new("Type the subtask to search for."));
        }
        Ok(serde_json::json!([query]))
    }

    fn call(&self, description: &str) -> Result<String, PathError> {
        self.arguments()?;
        Ok(format!(
            "spql.ai.similarity({description}, {}, model => {})",
            SqlLiteral::text(self.query().trim()),
            SqlLiteral::text(JUDGMENT_MODEL)
        ))
    }
}

/// The category each subtask fits best, and how probable that is.
impl SubtaskJudgment for SubtaskGrouping {
    const MEASURE: &'static str = SubtaskRow::CATEGORY;
    const FUNCTION: &'static str = "spql.ai.classify";

    fn arguments(&self) -> Result<serde_json::Value, PathError> {
        if self.categories().is_empty() {
            return Err(PathError::new(
                "Generate the categories, or write them, before grouping.",
            ));
        }
        Ok(self
            .categories()
            .iter()
            .map(|category| serde_json::json!([category.name(), category.definition()]))
            .collect())
    }

    fn call(&self, description: &str) -> Result<String, PathError> {
        self.arguments()?;
        let categories = self
            .categories()
            .iter()
            .map(|category| {
                format!(
                    "STRUCT_PACK(name := {}, definition := {})",
                    SqlLiteral::text(category.name()),
                    SqlLiteral::text(category.definition())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "spql.ai.classify({description}, [{categories}], model => {})",
            SqlLiteral::text(JUDGMENT_MODEL)
        ))
    }
}
