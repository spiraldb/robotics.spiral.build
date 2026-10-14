//! The Decisions requests the judgments send to jev, and how its answers are
//! read.
//!
//! A request is one `state`, the material every question judges, and a map
//! of question id to question. jev evaluates the questions of a request in
//! parallel and answers each under its id. What bounds a batch is the
//! request itself: 64k tokens, of which the state and the longest question
//! may use 32k. Both are measured on the JSON that is sent, at the four
//! characters to a token TypeSafe's own sizing uses.
//!
//! A subtask answers under its place in the episode's list (`r17`), so an
//! answer always names the subtask it describes, whichever request carried
//! it.
use serde::Deserialize;
use serde_json::{Map, Value, json};
use spiral_robotics_common::curation::CategoryDefinition;
use std::collections::BTreeMap;
use std::ops::Range;

/// Kept well inside jev's 64k-token request so a long text cannot push a
/// batch over it.
const REQUEST_CHARS: usize = 40_000 * 4;
/// Kept inside the 32k tokens jev allows the state and its longest question.
const STATE_CHARS: usize = 28_000 * 4;
/// jev's own limit on the options of one Choice.
const MAX_CATEGORIES: usize = 255;

/// One Decisions request and the rows its answers describe.
#[derive(Debug)]
pub(in crate::curation) struct QuestionBatch {
    state: Value,
    questions: Value,
    rows: Vec<usize>,
}

impl QuestionBatch {
    /// The request body for `model`.
    pub(super) fn request(&self, model: &str) -> Value {
        json!({"model": model, "state": self.state, "questions": self.questions})
    }

    /// The rows this request asks about.
    pub(super) fn rows(&self) -> &[usize] {
        &self.rows
    }

    /// The state key and question id one row answers under.
    fn row_key(row: usize) -> String {
        format!("r{row}")
    }

    /// Group `rows`, each `(row, text)`, into requests within both budgets.
    /// `head` is the state every request carries, `section` the state object
    /// that holds the rows' texts.
    fn plan(
        head: &Map<String, Value>,
        section: &str,
        rows: &[(usize, &str)],
        question: impl Fn(&str) -> Value,
    ) -> Vec<Self> {
        // Keys and values are measured as the JSON they become; the margins
        // cover quotes, separators, and the request's model and wrapping
        // fields.
        let costs = rows
            .iter()
            .map(|(row, text)| {
                let key = Self::row_key(*row);
                RowCost {
                    state: key.len() + Value::from(*text).to_string().len() + 4,
                    question: key.len() + question(&key).to_string().len() + 4,
                }
            })
            .collect::<Vec<_>>();
        let fixed = Value::Object(head.clone()).to_string().len() + section.len() + 128;
        BatchPlan { fixed }
            .groups(&costs)
            .into_iter()
            .map(|group| {
                let mut texts = Map::new();
                let mut questions = Map::new();
                for (row, text) in &rows[group.clone()] {
                    let key = Self::row_key(*row);
                    texts.insert(key.clone(), Value::String((*text).to_owned()));
                    questions.insert(key.clone(), question(&key));
                }
                let mut state = head.clone();
                state.insert(section.to_owned(), Value::Object(texts));
                Self {
                    state: Value::Object(state),
                    questions: Value::Object(questions),
                    rows: rows[group].iter().map(|(row, _)| *row).collect(),
                }
            })
            .collect()
    }
}

/// What one row adds to a request's state and to its questions.
struct RowCost {
    state: usize,
    question: usize,
}

/// Where one request ends and the next begins.
struct BatchPlan {
    /// What every request carries before any row.
    fixed: usize,
}

impl BatchPlan {
    /// Contiguous row groups, each within both budgets. A row too large for
    /// an empty request still gets its own group, so it is asked about and
    /// fails on its own if jev refuses it.
    fn groups(&self, costs: &[RowCost]) -> Vec<Range<usize>> {
        let mut groups = Vec::new();
        let mut start = 0;
        let (mut state, mut request) = (self.fixed, self.fixed);
        for (row, cost) in costs.iter().enumerate() {
            let full = row > start
                && (state + cost.state > STATE_CHARS
                    || request + cost.state + cost.question > REQUEST_CHARS);
            if full {
                groups.push(start..row);
                start = row;
                (state, request) = (self.fixed, self.fixed);
            }
            state += cost.state;
            request += cost.state + cost.question;
        }
        if start < costs.len() {
            groups.push(start..costs.len());
        }
        groups
    }
}

/// One way of questioning jev about the texts of rows that share it.
pub(in crate::curation) trait Judgment {
    type Answer;

    /// The requests that ask about `rows`, each `(row, text)`.
    fn batches(&self, rows: &[(usize, &str)]) -> Vec<QuestionBatch>;

    /// One row's answer out of its request's answers.
    fn read(&self, answers: &DecisionAnswers, row: usize) -> Result<Self::Answer, String>;
}

/// The Noul requests that ask whether each text describes the same
/// operation as one query, following TypeSafe's re-ranking cookbook.
pub(in crate::curation) struct SimilarityQuestions {
    head: Map<String, Value>,
}

impl SimilarityQuestions {
    pub(in crate::curation) fn new(query: &str) -> Result<Self, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("the similarity query is empty".to_owned());
        }
        let mut head = Map::new();
        head.insert("query".to_owned(), Value::String(query.to_owned()));
        Ok(Self { head })
    }

    fn question(key: &str) -> Value {
        json!({
            "type": "noul",
            "instructions": format!(
                "Does `candidates.{key}` describe the same manipulation operation as `query`?"
            ),
            "criteria": {
                "true": "the same operation on the same kind of object, even if worded or executed differently.",
                "false": "a different operation, a neighboring step, or only the same object or scene.",
            },
        })
    }
}

impl Judgment for SimilarityQuestions {
    type Answer = f64;

    fn batches(&self, rows: &[(usize, &str)]) -> Vec<QuestionBatch> {
        QuestionBatch::plan(&self.head, "candidates", rows, Self::question)
    }

    fn read(&self, answers: &DecisionAnswers, row: usize) -> Result<f64, String> {
        answers.noul(row)
    }
}

/// The Choice requests that put each text in one category. Every question
/// offers the same criteria, each category's name mapped to its definition.
pub(in crate::curation) struct CategoryQuestions {
    criteria: BTreeMap<String, String>,
}

impl CategoryQuestions {
    /// `categories`: at least one, at most jev's 255, with distinct names and
    /// nothing blank.
    pub(in crate::curation) fn new(categories: &[CategoryDefinition]) -> Result<Self, String> {
        if categories.is_empty() {
            return Err("classify needs at least one category".to_owned());
        }
        if categories.len() > MAX_CATEGORIES {
            return Err(format!(
                "classify accepts at most {MAX_CATEGORIES} categories, got {}",
                categories.len()
            ));
        }
        let mut criteria = BTreeMap::new();
        for category in categories {
            let (name, definition) = (category.name().trim(), category.definition().trim());
            if name.is_empty() || definition.is_empty() {
                return Err("every category needs a name and a definition".to_owned());
            }
            if criteria
                .insert(name.to_owned(), definition.to_owned())
                .is_some()
            {
                return Err(format!("two categories are named {name:?}"));
            }
        }
        Ok(Self { criteria })
    }

    fn question(&self, key: &str) -> Value {
        json!({
            "type": "choice",
            "instructions": format!("Which category does `subtasks.{key}` belong to?"),
            "criteria": self.criteria,
        })
    }

    fn offers(&self, category: &str) -> bool {
        self.criteria.contains_key(category)
    }
}

impl Judgment for CategoryQuestions {
    type Answer = (String, f64);

    fn batches(&self, rows: &[(usize, &str)]) -> Vec<QuestionBatch> {
        let mut head = Map::new();
        head.insert("categories".to_owned(), json!(self.criteria));
        QuestionBatch::plan(&head, "subtasks", rows, |key| self.question(key))
    }

    fn read(&self, answers: &DecisionAnswers, row: usize) -> Result<(String, f64), String> {
        answers.choice(row, self)
    }
}

/// jev's answers to one request, keyed by question id.
#[derive(Debug, Deserialize)]
pub(in crate::curation) struct DecisionAnswers {
    answers: BTreeMap<String, DecisionAnswer>,
}

#[derive(Debug, Deserialize)]
struct DecisionAnswer {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    noul: Option<f64>,
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    probabilities: BTreeMap<String, f64>,
}

impl DecisionAnswers {
    pub(super) fn parse(answer: Value) -> Result<Self, String> {
        serde_json::from_value(answer)
            .map_err(|error| format!("jev answered without the Decisions answer map: {error}"))
    }

    /// A Noul answer: the probability, 0 to 1, that the row's question
    /// holds.
    fn noul(&self, row: usize) -> Result<f64, String> {
        let answer = self.answer(row, "noul")?;
        answer
            .noul
            .filter(|probability| Self::is_probability(*probability))
            .ok_or_else(|| "jev answered this row without a probability".to_owned())
    }

    /// A Choice answer: the winning category and its probability.
    fn choice(&self, row: usize, questions: &CategoryQuestions) -> Result<(String, f64), String> {
        let answer = self.answer(row, "choice")?;
        let category = answer
            .choice
            .as_deref()
            .filter(|category| questions.offers(category))
            .ok_or_else(|| "jev chose no offered category for this row".to_owned())?;
        let probability = answer
            .probabilities
            .get(category)
            .copied()
            .filter(|probability| Self::is_probability(*probability))
            .ok_or_else(|| "jev gave no probability for its chosen category".to_owned())?;
        Ok((category.to_owned(), probability))
    }

    fn answer(&self, row: usize, kind: &str) -> Result<&DecisionAnswer, String> {
        self.answers
            .get(&QuestionBatch::row_key(row))
            .filter(|answer| answer.kind == kind)
            .ok_or_else(|| format!("jev returned no {kind} answer for this row"))
    }

    fn is_probability(value: f64) -> bool {
        value.is_finite() && (0.0..=1.0).contains(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(count: usize, width: usize) -> Vec<String> {
        (0..count)
            .map(|row| format!("{row} {}", "fold the cloth ".repeat(width)))
            .collect()
    }

    fn rows(texts: &[String]) -> Vec<(usize, &str)> {
        texts.iter().map(String::as_str).enumerate().collect()
    }

    fn covers(batches: &[QuestionBatch], rows: usize) {
        let asked = batches
            .iter()
            .flat_map(|batch| batch.rows().iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(asked, (0..rows).collect::<Vec<_>>());
    }

    fn categories(pairs: &[(&str, &str)]) -> Vec<CategoryDefinition> {
        pairs
            .iter()
            .map(|(name, definition)| CategoryDefinition::new(*name, *definition))
            .collect()
    }

    #[test]
    fn a_similarity_question_names_its_row_in_state_and_instructions() {
        let table = texts(3, 1);
        let batches = SimilarityQuestions::new(" make the first fold ")
            .unwrap()
            .batches(&[(7, table[0].as_str()), (9, table[2].as_str())]);
        assert_eq!(batches.len(), 1);
        let request = batches[0].request("typesafe/jev-1.13");
        assert_eq!(request["model"], "typesafe/jev-1.13");
        assert_eq!(request["state"]["query"], "make the first fold");
        assert_eq!(request["state"]["candidates"]["r9"], table[2].as_str());
        assert_eq!(request["questions"]["r7"]["type"], "noul");
        assert_eq!(
            request["questions"]["r9"]["instructions"],
            "Does `candidates.r9` describe the same manipulation operation as `query`?"
        );
        assert_eq!(batches[0].rows(), [7, 9]);
        assert!(SimilarityQuestions::new("  ").is_err());
    }

    #[test]
    fn batches_stay_within_the_request_and_state_budgets() {
        // Long texts fill the state budget first; many short ones fill the
        // request budget.
        for table in [texts(400, 200), texts(6_000, 0)] {
            let batches = SimilarityQuestions::new("fold")
                .unwrap()
                .batches(&rows(&table));
            assert!(batches.len() > 1);
            covers(&batches, table.len());
            for batch in &batches {
                let request = batch.request("m");
                assert!(request["state"].to_string().len() <= STATE_CHARS);
                assert!(request.to_string().len() <= REQUEST_CHARS);
            }
        }
        let oversized = ["x".repeat(REQUEST_CHARS * 2)];
        let batches = SimilarityQuestions::new("fold")
            .unwrap()
            .batches(&rows(&oversized));
        assert_eq!(
            batches.len(),
            1,
            "a row too large for a request is still asked"
        );
    }

    #[test]
    fn category_questions_offer_every_definition_and_refuse_bad_sets() {
        let offered = categories(&[("fold", "Folding the cloth."), ("grasp", "Taking hold.")]);
        let questions = CategoryQuestions::new(&offered).unwrap();
        let request = questions.batches(&[(0, "make the first fold")])[0].request("m");
        assert_eq!(request["state"]["categories"]["grasp"], "Taking hold.");
        assert_eq!(request["state"]["subtasks"]["r0"], "make the first fold");
        assert_eq!(
            request["questions"]["r0"]["instructions"],
            "Which category does `subtasks.r0` belong to?"
        );
        assert_eq!(
            request["questions"]["r0"]["criteria"]["fold"],
            "Folding the cloth."
        );
        assert!(CategoryQuestions::new(&[]).is_err());
        assert!(CategoryQuestions::new(&categories(&[("fold", "a"), (" fold ", "b")])).is_err());
        assert!(CategoryQuestions::new(&categories(&[("fold", " ")])).is_err());
    }

    #[test]
    fn only_well_formed_answers_of_the_asked_kind_are_read() {
        let questions =
            CategoryQuestions::new(&categories(&[("fold", "a"), ("grasp", "b")])).unwrap();
        let answers = DecisionAnswers::parse(json!({"answers": {
            "r0": {"type": "noul", "noul": 0.83},
            "r1": {"type": "noul", "noul": 1.5},
            "r2": {"type": "choice", "choice": "grasp", "confidence": 0.4,
                   "probabilities": {"fold": 0.3, "grasp": 0.7}},
            "r3": {"type": "choice", "choice": "smooth", "probabilities": {"smooth": 1.0}},
        }}))
        .unwrap();
        assert_eq!(answers.noul(0), Ok(0.83));
        assert!(answers.noul(1).is_err());
        assert!(answers.noul(2).is_err());
        assert!(answers.noul(4).is_err());
        assert_eq!(answers.choice(2, &questions), Ok(("grasp".to_owned(), 0.7)));
        assert!(answers.choice(3, &questions).is_err());
        assert!(DecisionAnswers::parse(json!({"usage": {}})).is_err());
    }
}
