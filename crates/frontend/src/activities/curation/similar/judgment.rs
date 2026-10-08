//! What Find similar and Group similar subtasks add to a path: one text
//! judgment of every subtask of the Break down above.
//!
//! Find similar asks `spql.ai.similarity` against its query, and Group
//! similar `spql.ai.classify` over its categories, written into the
//! statement as a literal list. A fill judges each subtask the Break down
//! above stored for an episode and collects the answers into one list
//! aligned with them; the published view and the preview read each
//! subtask's judgment at its place in that list. Find similar folds every
//! subtask with its similarity into one column, and Group similar the
//! subtasks of each category into a column of its own, named by the
//! category's id.
use crate::activities::curation::annotate::SubtaskListing;
use crate::activities::curation::document::{SubtaskGrouping, SubtaskSearch};
use crate::activities::curation::path::{
    ColumnMeaning, EpisodeRow, ModelEntries, ModelNode, ModelStep, PathError, TableColumn,
};
use spiral_robotics_common::curation::{AnswerColumn, AnswerSource, AnswerTable};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::sql::SqlLiteral;

/// The decision model both judgments are asked of, pinned so every published
/// answer names the model that gave it.
const JUDGMENT_MODEL: &str = "typesafe/jev-1.13";

/// A text judgment of every subtask in a list, as either node asks it.
trait SubtaskJudgment {
    /// What the node measures, which names its stored column.
    const MEASURE: &'static str;
    /// The function its call is to, which names its stored column.
    const FUNCTION: &'static str;
    /// What its entries read of each subtask's judgment: every field as its
    /// expression over `judgment` and its name.
    const JUDGED: &'static [(&'static str, &'static str)];
    /// Everything besides the subtasks that changes the answer, or why the
    /// node cannot ask yet.
    fn arguments(&self) -> Result<serde_json::Value, PathError>;
    /// The call judging one subtask's `description`.
    fn call(&self, description: &str) -> Result<String, PathError>;
    /// Its columns folded from its entries.
    fn fold(&self, entries: ModelEntries, step: &ModelStep<'_>) -> ModelEntries;

    /// The column the judgments are stored in: its function, its model, what
    /// it asks, and the Break down column whose subtasks it judges.
    fn column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        let subtasks = subtasks.ok_or_else(|| {
            PathError::new("This node reads the subtasks a Break down above it describes.")
        })?;
        let call = serde_json::json!([
            Self::FUNCTION,
            JUDGMENT_MODEL,
            self.arguments()?,
            subtasks.as_str()
        ]);
        AnswerColumn::of(Self::MEASURE, &call).map_err(PathError::new)
    }
}

/// Every subtask of the Break down above, with its judgment read from the
/// stored list at the subtask's place.
impl<J: SubtaskJudgment> ModelNode for J {
    fn answer_column(&self, subtasks: Option<&AnswerColumn>) -> Result<AnswerColumn, PathError> {
        self.column(subtasks)
    }

    /// A fill judges, for the episode bound as `$1`, each subtask the Break
    /// down above stored, collected in their order.
    fn answer_source(
        &self,
        dataset: &DatasetSelection,
        step: &ModelStep<'_>,
    ) -> Result<AnswerSource, PathError> {
        let table = AnswerTable::of(dataset).map_err(PathError::new)?;
        let call = self.call("s.listed.description")?;
        Ok(AnswerSource::Statement {
            statement: format!(
                "WITH e AS (SELECT a.episode_id, a.{}.value AS subtasks FROM {} a WHERE a.episode_id = $1), \
                 s AS (SELECT e.episode_id, spql.list.get(e.subtasks, g._) AS listed \
                 FROM e CROSS JOIN LATERAL spql.range(CAST(spql.list.len(e.subtasks) AS BIGINT)) g) \
                 SELECT s.episode_id, spql.list.collect({call}) AS answer FROM s GROUP BY s.episode_id",
                step.subtasks()?,
                table.quoted()
            ),
        })
    }

    fn episode_columns(&self, row: &mut EpisodeRow, step: &ModelStep<'_>) -> Result<(), PathError> {
        row.stored(step.subtasks()?)?;
        row.stored(step.column())
    }

    fn entries(&self, step: &ModelStep<'_>) -> Result<ModelEntries, PathError> {
        Ok(SubtaskListing::new(step, step.subtasks()?)
            .judged_as(Self::JUDGED)
            .entries(|entries| self.fold(entries, step)))
    }
}

/// How likely each subtask is the operation the query names.
impl SubtaskJudgment for SubtaskSearch {
    const MEASURE: &'static str = "similarity";
    const FUNCTION: &'static str = "spql.ai.similarity";
    const JUDGED: &'static [(&'static str, &'static str)] = &[("judgment", "similarity")];

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

    /// Every subtask with its similarity, in one column.
    fn fold(&self, entries: ModelEntries, step: &ModelStep<'_>) -> ModelEntries {
        entries.folding(
            TableColumn::of_node(step.node(), &step.name(), ColumnMeaning::Similarities),
            (
                "STRUCT_PACK(start_ns := start_ns, end_ns := end_ns, similarity := similarity)"
                    .to_owned(),
                None,
            ),
        )
    }
}

/// The category each subtask fits best, and how probable that is.
impl SubtaskJudgment for SubtaskGrouping {
    const MEASURE: &'static str = "subtask_category";
    const FUNCTION: &'static str = "spql.ai.classify";
    const JUDGED: &'static [(&'static str, &'static str)] = &[
        ("judgment.category", "category"),
        ("judgment.probability", "probability"),
    ];

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

    /// The subtasks of each category, in a column of its own named by the
    /// category's id: the classifier answers by name, which the statement
    /// matches.
    fn fold(&self, entries: ModelEntries, step: &ModelStep<'_>) -> ModelEntries {
        self.categories().iter().fold(entries, |entries, category| {
            entries.folding(
                TableColumn::of_node(
                    step.node(),
                    &step.name(),
                    ColumnMeaning::Category(category.id()),
                ),
                (
                    "STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)".to_owned(),
                    Some(format!("category = {}", SqlLiteral::text(category.name()))),
                ),
            )
        })
    }
}
