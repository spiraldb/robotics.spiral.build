//! The steps Find similar and Group similar subtasks add to a path: one text
//! judgment of every subtask's description that reaches the node.
//!
//! Find similar compiles to `spql.ai.similarity` against its query, and Group
//! similar to `spql.ai.classify` over its categories, written into the
//! statement as a literal list. Both hand on every subtask with the judgment
//! beside it, for a Select below to keep part of.
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{SubtaskGrouping, SubtaskSearch};
use crate::activities::curation::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use spiral_robotics_common::sql::SqlLiteral;

/// The decision model both judgments are asked of, pinned so every published
/// answer names the model that gave it.
const JUDGMENT_MODEL: &str = "typesafe/jev-1.13";

/// Where one judgment is written: what reaches the node, the description
/// it reads, the column it writes, and the relation it hands on.
struct JudgedStep<'a> {
    name: &'a str,
    input: &'a Relation,
    description: &'a str,
    column: String,
    relation: Relation,
}

impl<'a> JudgedStep<'a> {
    /// The step writing `measure` over what reaches `step`, which must carry
    /// the subtasks a Break down above described.
    fn of(step: &'a PathStep<'_>, measure: &'static str) -> Result<Self, PathError> {
        let input = step.input()?;
        let description = input.measured(SubtaskRow::DESCRIPTION).ok_or_else(|| {
            PathError::new("This node reads the subtasks a Break down above it describes.")
        })?;
        let relation = input.measuring(step.name(), measure);
        let column = relation
            .measured(measure)
            .expect("the relation carries what it just measured")
            .to_owned();
        Ok(Self {
            name: step.name(),
            input,
            description,
            column,
            relation,
        })
    }
    /// The one entry: every column that reached the node, then `call` of the
    /// description as the new column.
    fn sql(self, call: impl FnOnce(&str) -> String) -> StepSql {
        let entry = format!(
            "{} AS (SELECT {}, {} AS {} FROM {})",
            self.name,
            self.input.columns(),
            call(self.description),
            self.column,
            self.input.name()
        );
        StepSql::new(vec![entry], self.relation)
    }
}

/// Every subtask that reaches the node, with how likely it is the operation
/// the query names.
impl PathFragment for SubtaskSearch {
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let query = self.query().trim();
        if query.is_empty() {
            return Err(PathError::new("Type the subtask to search for."));
        }
        let judged = JudgedStep::of(step, SubtaskRow::SIMILARITY)?;
        Ok(judged.sql(|description| {
            format!(
                "spql.ai.similarity({description}, {}, model => {}{})",
                SqlLiteral::text(query),
                SqlLiteral::text(JUDGMENT_MODEL),
                step.source().on_error()
            )
        }))
    }
}

/// Every subtask that reaches the node, with the category it fits best and
/// how probable that is.
impl PathFragment for SubtaskGrouping {
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        if self.categories().is_empty() {
            return Err(PathError::new(
                "Generate the categories, or write them, before grouping.",
            ));
        }
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
        let judged = JudgedStep::of(step, SubtaskRow::CATEGORY)?;
        Ok(judged.sql(|description| {
            format!(
                "spql.ai.classify({description}, [{categories}], model => {}{})",
                SqlLiteral::text(JUDGMENT_MODEL),
                step.source().on_error()
            )
        }))
    }
}
