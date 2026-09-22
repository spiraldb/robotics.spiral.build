//! The Decisions requests both explorations send: one state, one question
//! per subtask row, split into the batches a single request can carry.
//!
//! jev evaluates every question of a request in parallel against the same
//! state, so asking about many rows at once costs little latency. What
//! bounds a batch is the request itself — 64k tokens, 32k of them state —
//! and the 128 KB of response `ModelClient::decide` reads back. Both are
//! measured here on the JSON that will actually be sent, at the four
//! characters to a token TypeSafe's own sizing uses.
//!
//! A row keeps its index in the whole table as its question id (`r17`), so
//! a batch's answers always name the row they describe, whichever batch
//! carried them.
use crate::activities::workflow::build::document::SubtaskCategory;
use serde_json::{Map, Value, json};
use std::ops::Range;

/// Kept well inside jev's 64k-token request so a long description cannot
/// push a batch over it.
const REQUEST_CHARS: usize = 40_000 * 4;
/// `ModelClient::decide` accepts 128 KB of response; leave it headroom.
const RESPONSE_BYTES: usize = 100 * 1024;
/// What one `"r123":{"type":"noul","noul":0.12345678}` answer weighs.
const NOUL_ANSWER_BYTES: usize = 48;
/// A Choice answer without its per-option probabilities.
const CHOICE_ANSWER_BYTES: usize = 96;
/// One option inside a Choice answer's probabilities, beside its name.
const CHOICE_OPTION_BYTES: usize = 12;
/// jev's own limit on a Choice.
const MAX_CHOICE_OPTIONS: usize = 255;

/// One Decisions request: what to send, and the rows its answers describe.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct QuestionBatch {
    state: Value,
    questions: Value,
    rows: Range<usize>,
}
impl QuestionBatch {
    pub(super) fn state(&self) -> &Value {
        &self.state
    }
    pub(super) fn questions(&self) -> &Value {
        &self.questions
    }
    /// The rows of the whole table this request asks about.
    pub(super) fn rows(&self) -> Range<usize> {
        self.rows.clone()
    }
    /// The state key and question id one row answers under.
    pub(super) fn row_key(row: usize) -> String {
        format!("r{row}")
    }
}

/// Where one request ends and the next begins.
struct BatchPlan {
    /// What every request of this kind carries before any row.
    fixed_chars: usize,
    /// What one row's answer weighs in the response.
    answer_bytes: usize,
}
impl BatchPlan {
    /// Contiguous row groups, each within both budgets. A row too large for
    /// an empty request still gets its own group: refusing to ask about it
    /// would silently drop it from a ranking that promises every row.
    fn groups(&self, row_chars: &[usize]) -> Vec<Range<usize>> {
        let rows_per_response = (RESPONSE_BYTES / self.answer_bytes.max(1)).max(1);
        let mut groups = Vec::new();
        let mut start = 0;
        let mut chars = self.fixed_chars;
        for (row, cost) in row_chars.iter().enumerate() {
            let full =
                row > start && (chars + cost > REQUEST_CHARS || row - start >= rows_per_response);
            if full {
                groups.push(start..row);
                start = row;
                chars = self.fixed_chars;
            }
            chars += cost;
        }
        if start < row_chars.len() {
            groups.push(start..row_chars.len());
        }
        groups
    }
}

/// The Noul requests that rank a subtask table against one query, one
/// question per candidate row, following TypeSafe's re-ranking cookbook.
pub(super) struct SearchQuestions {
    query: String,
    /// The dataset's task instruction, when the document knows one.
    task: Option<String>,
}
impl SearchQuestions {
    pub(super) fn new(query: impl Into<String>, task: Option<String>) -> Result<Self, String> {
        let query = query.into().trim().to_owned();
        if query.is_empty() {
            return Err("Type a subtask to search for, or pick one from the table.".into());
        }
        Ok(Self {
            query,
            task: task
                .map(|task| task.trim().to_owned())
                .filter(|task| !task.is_empty()),
        })
    }
    pub(super) fn batches(&self, descriptions: &[&str]) -> Vec<QuestionBatch> {
        let row_chars = descriptions
            .iter()
            .enumerate()
            .map(|(row, description)| Self::row_chars(row, description))
            .collect::<Vec<_>>();
        BatchPlan {
            fixed_chars: Value::Object(self.head()).to_string().len(),
            answer_bytes: NOUL_ANSWER_BYTES,
        }
        .groups(&row_chars)
        .into_iter()
        .map(|rows| self.batch(rows, descriptions))
        .collect()
    }
    /// The state every batch of this search carries.
    fn head(&self) -> Map<String, Value> {
        let mut head = Map::new();
        head.insert("query".into(), Value::String(self.query.clone()));
        if let Some(task) = &self.task {
            head.insert("task".into(), Value::String(task.clone()));
        }
        head
    }
    fn row_chars(row: usize, description: &str) -> usize {
        let key = QuestionBatch::row_key(row);
        Self::question(row).to_string().len() + key.len() * 2 + description.len() + 16
    }
    fn batch(&self, rows: Range<usize>, descriptions: &[&str]) -> QuestionBatch {
        let mut candidates = Map::new();
        let mut questions = Map::new();
        for row in rows.clone() {
            let key = QuestionBatch::row_key(row);
            candidates.insert(key.clone(), Value::String(descriptions[row].to_owned()));
            questions.insert(key, Self::question(row));
        }
        let mut state = self.head();
        state.insert("candidates".into(), Value::Object(candidates));
        QuestionBatch {
            state: Value::Object(state),
            questions: Value::Object(questions),
            rows,
        }
    }
    fn question(row: usize) -> Value {
        json!({
            "type": "noul",
            "instructions": format!(
                "Does `candidates.{}` describe the same manipulation operation as `query`?",
                QuestionBatch::row_key(row)
            ),
            "criteria": {
                "true": "the same operation on the same kind of object, even if worded or executed differently.",
                "false": "a different operation, a neighboring step, or only the same object or scene."
            }
        })
    }
}

/// The Choice requests that put every subtask row in one category. Every
/// question carries the same criteria — each category name mapped to its
/// definition — so a batch pays for them once per row.
pub(super) struct AssignmentQuestions {
    criteria: Map<String, Value>,
}
impl AssignmentQuestions {
    pub(super) fn new(categories: &[SubtaskCategory]) -> Result<Self, String> {
        if categories.is_empty() {
            return Err("Generate categories before assigning rows.".into());
        }
        if categories.len() > MAX_CHOICE_OPTIONS {
            return Err(format!(
                "A Choice holds at most {MAX_CHOICE_OPTIONS} categories."
            ));
        }
        let criteria = categories
            .iter()
            .map(|category| {
                (
                    category.name().to_owned(),
                    Value::String(category.definition().to_owned()),
                )
            })
            .collect::<Map<_, _>>();
        if criteria.len() != categories.len() {
            return Err("Two categories share a name.".into());
        }
        Ok(Self { criteria })
    }
    pub(super) fn batches(&self, descriptions: &[&str]) -> Vec<QuestionBatch> {
        let row_chars = descriptions
            .iter()
            .enumerate()
            .map(|(row, description)| self.row_chars(row, description))
            .collect::<Vec<_>>();
        BatchPlan {
            fixed_chars: Value::Object(self.head()).to_string().len(),
            answer_bytes: self.answer_bytes(),
        }
        .groups(&row_chars)
        .into_iter()
        .map(|rows| self.batch(rows, descriptions))
        .collect()
    }
    fn head(&self) -> Map<String, Value> {
        let mut head = Map::new();
        head.insert("categories".into(), Value::Object(self.criteria.clone()));
        head
    }
    fn row_chars(&self, row: usize, description: &str) -> usize {
        let key = QuestionBatch::row_key(row);
        self.question(row).to_string().len() + key.len() * 2 + description.len() + 16
    }
    fn answer_bytes(&self) -> usize {
        CHOICE_ANSWER_BYTES
            + self
                .criteria
                .keys()
                .map(|name| name.len() + CHOICE_OPTION_BYTES)
                .sum::<usize>()
    }
    fn batch(&self, rows: Range<usize>, descriptions: &[&str]) -> QuestionBatch {
        let mut subtasks = Map::new();
        let mut questions = Map::new();
        for row in rows.clone() {
            let key = QuestionBatch::row_key(row);
            subtasks.insert(key.clone(), Value::String(descriptions[row].to_owned()));
            questions.insert(key, self.question(row));
        }
        let mut state = self.head();
        state.insert("subtasks".into(), Value::Object(subtasks));
        QuestionBatch {
            state: Value::Object(state),
            questions: Value::Object(questions),
            rows,
        }
    }
    fn question(&self, row: usize) -> Value {
        json!({
            "type": "choice",
            "instructions": format!(
                "Which category does `subtasks.{}` belong to?",
                QuestionBatch::row_key(row)
            ),
            "criteria": Value::Object(self.criteria.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptions(count: usize, width: usize) -> Vec<String> {
        (0..count)
            .map(|row| format!("{row} {}", "fold the cloth ".repeat(width)))
            .collect()
    }
    fn borrowed(descriptions: &[String]) -> Vec<&str> {
        descriptions.iter().map(String::as_str).collect()
    }
    fn request_chars(batch: &QuestionBatch) -> usize {
        batch.state().to_string().len() + batch.questions().to_string().len()
    }
    fn covers(batches: &[QuestionBatch], rows: usize) {
        let mut next = 0;
        for batch in batches {
            assert_eq!(batch.rows().start, next);
            next = batch.rows().end;
        }
        assert_eq!(next, rows);
    }

    #[test]
    fn a_small_table_asks_one_question_per_row_in_one_request() {
        let table = descriptions(3, 1);
        let batches = SearchQuestions::new("make the first fold", None)
            .unwrap()
            .batches(&borrowed(&table));
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.rows(), 0..3);
        assert_eq!(batch.state()["query"], "make the first fold");
        assert!(batch.state().get("task").is_none());
        assert_eq!(batch.state()["candidates"]["r2"], table[2].as_str());
        assert_eq!(batch.questions()["r2"]["type"], "noul");
        assert_eq!(
            batch.questions()["r2"]["instructions"],
            "Does `candidates.r2` describe the same manipulation operation as `query`?"
        );
        assert_eq!(
            batch.questions()["r0"]["criteria"]["true"],
            "the same operation on the same kind of object, even if worded or executed differently."
        );
    }

    #[test]
    fn a_known_task_instruction_joins_every_batch_state() {
        let table = descriptions(2, 1);
        let batches = SearchQuestions::new("fold", Some("  fold the towel  ".into()))
            .unwrap()
            .batches(&borrowed(&table));
        assert_eq!(batches[0].state()["task"], "fold the towel");
        assert!(
            SearchQuestions::new("fold", Some("   ".into()))
                .unwrap()
                .batches(&borrowed(&table))[0]
                .state()
                .get("task")
                .is_none()
        );
    }

    #[test]
    fn an_empty_query_is_refused_and_an_empty_table_asks_nothing() {
        assert!(SearchQuestions::new("   ", None).is_err());
        assert!(
            SearchQuestions::new("fold", None)
                .unwrap()
                .batches(&[])
                .is_empty()
        );
    }

    #[test]
    fn long_descriptions_split_batches_at_the_request_budget() {
        let table = descriptions(400, 64);
        let batches = SearchQuestions::new("fold", None)
            .unwrap()
            .batches(&borrowed(&table));
        assert!(batches.len() > 1, "wide rows must not ride in one request");
        covers(&batches, table.len());
        for batch in &batches {
            assert!(request_chars(batch) <= REQUEST_CHARS);
        }
    }

    #[test]
    fn a_long_table_of_short_descriptions_stays_within_both_budgets() {
        let table = descriptions(6_000, 0);
        let batches = SearchQuestions::new("fold", None)
            .unwrap()
            .batches(&borrowed(&table));
        covers(&batches, table.len());
        for batch in &batches {
            assert!(request_chars(batch) <= REQUEST_CHARS);
            assert!(batch.rows().len() * NOUL_ANSWER_BYTES <= RESPONSE_BYTES);
        }
        // One Noul question is ~330 characters, so the request budget is
        // what a search hits first: about 400 candidates a request, as the
        // design doc sizes it.
        assert!(
            (300..600).contains(&batches[0].rows().len()),
            "{}",
            batches[0].rows().len()
        );
    }

    #[test]
    fn the_response_cap_bounds_a_batch_the_request_budget_would_not() {
        let groups = BatchPlan {
            fixed_chars: 0,
            answer_bytes: NOUL_ANSWER_BYTES,
        }
        .groups(&[1; 10_000]);
        assert_eq!(groups[0], 0..RESPONSE_BYTES / NOUL_ANSWER_BYTES);
        assert_eq!(groups.last().expect("one group").end, 10_000);
    }

    #[test]
    fn one_row_too_large_for_a_request_still_gets_asked() {
        let table = vec![
            "short".to_owned(),
            "x".repeat(REQUEST_CHARS * 2),
            "short".to_owned(),
        ];
        let batches = SearchQuestions::new("fold", None)
            .unwrap()
            .batches(&borrowed(&table));
        covers(&batches, table.len());
        assert_eq!(batches[1].rows(), 1..2);
    }

    fn category(name: &str) -> SubtaskCategory {
        SubtaskCategory::new(name, format!("Everything that {name}s the cloth.")).unwrap()
    }

    #[test]
    fn assignment_questions_offer_every_category_to_every_row() {
        let categories = [category("fold"), category("grasp"), category("smooth")];
        let table = descriptions(2, 1);
        let batches = AssignmentQuestions::new(&categories)
            .unwrap()
            .batches(&borrowed(&table));
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(
            batch.state()["categories"]["fold"],
            categories[0].definition()
        );
        assert_eq!(batch.state()["subtasks"]["r1"], table[1].as_str());
        assert_eq!(batch.questions()["r1"]["type"], "choice");
        assert_eq!(
            batch.questions()["r1"]["instructions"],
            "Which category does `subtasks.r1` belong to?"
        );
        assert_eq!(
            batch.questions()["r0"]["criteria"]["smooth"],
            categories[2].definition()
        );
    }

    #[test]
    fn repeating_the_criteria_per_row_shrinks_assignment_batches() {
        let categories = (0..8)
            .map(|index| {
                SubtaskCategory::new(
                    format!("category {index}"),
                    "A one-sentence definition that distinguishes this category. ".repeat(3),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let table = descriptions(2_000, 2);
        let batches = AssignmentQuestions::new(&categories)
            .unwrap()
            .batches(&borrowed(&table));
        assert!(batches.len() > 1);
        covers(&batches, table.len());
        for batch in &batches {
            assert!(request_chars(batch) <= REQUEST_CHARS);
        }
    }

    #[test]
    fn assignment_needs_categories_and_refuses_duplicates() {
        assert!(AssignmentQuestions::new(&[]).is_err());
        assert!(AssignmentQuestions::new(&[category("fold"), category("fold")]).is_err());
    }
}
