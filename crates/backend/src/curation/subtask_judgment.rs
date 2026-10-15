//! Find similar's and Group similar's judgments of the subtasks a Break down
//! stored for one episode.
//!
//! Both are answered by jev, TypeSafe's decision model, through
//! OpenRouter's Decisions API, which evaluates many typed questions against
//! one JSON state in parallel: an episode's subtasks share their questions —
//! one query, or one category list — so they are asked about together, as
//! many to a request as it can carry. The answer is one list per episode,
//! aligned with its subtasks: Find similar's the probability, 0 to 1, that
//! a subtask describes the same manipulation operation on the same kind of
//! object as the query, even if worded or executed differently; Group
//! similar's the most probable of the offered categories, `{category,
//! probability}`, there being no "other". A subtask jev cannot answer, or
//! one with no description, is NULL in the list; an episode Break down
//! stored no subtasks for is the episode's failed answer; a refused key ends
//! the run.
mod decisions;

pub(super) use decisions::{CategoryQuestions, SimilarityQuestions};

use super::fill::Answered;
use super::openrouter::{OpenRouter, ProviderFailure};
use arrow_array::cast::AsArray as _;
use arrow_array::{Array as _, ArrayRef, Float64Array, RecordBatch, StringArray, StructArray};
use arrow_buffer::NullBuffer;
use arrow_schema::{DataType, Field, Fields};
use decisions::{DecisionAnswers, Judgment};
use futures_util::future::join_all;
use spiral_robotics_common::curation::{AnswerColumn, AnswerTable};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_storage::answers::EpisodeAnswer;
use std::sync::Arc;

/// The subtasks a Break down column stores for one episode, as a judgment
/// reads them: each one's description, in order; none when the column holds
/// none for the episode.
pub(super) struct StoredSubtasks(Option<Vec<Option<String>>>);

impl StoredSubtasks {
    /// The statement reading the subtasks `column` stores in `dataset`'s
    /// answers table for the episode bound as `$1`.
    pub(super) fn statement(dataset: &DatasetSelection, column: &AnswerColumn) -> String {
        format!(
            "SELECT a.{column}.value AS subtasks FROM {} a WHERE a.episode_id = $1",
            AnswerTable::of(dataset).table()
        )
    }

    /// What the statement read: its one row's list of subtasks.
    pub(super) fn read(batches: &[RecordBatch]) -> Result<Self, String> {
        let Some(batch) = batches.iter().find(|batch| batch.num_rows() > 0) else {
            return Ok(Self(None));
        };
        if batches.iter().map(RecordBatch::num_rows).sum::<usize>() > 1 {
            return Err("The answers table holds the episode more than once.".into());
        }
        let lists = batch
            .column_by_name("subtasks")
            .ok_or("The stored subtasks were not read.")?;
        let item = match lists.data_type() {
            DataType::List(item) | DataType::LargeList(item) => item.clone(),
            other => return Err(format!("A Break down column stores {other}, not subtasks.")),
        };
        let lists =
            arrow_cast::cast(lists, &DataType::List(item)).map_err(|error| error.to_string())?;
        let lists = lists.as_list::<i32>();
        if lists.is_null(0) || lists.value_length(0) == 0 {
            return Ok(Self(None));
        }
        let subtasks = lists.value(0);
        let descriptions = subtasks
            .as_struct_opt()
            .and_then(|subtasks| subtasks.column_by_name("description"))
            .ok_or("A Break down column stores subtasks without descriptions.")?;
        let descriptions =
            arrow_cast::cast(descriptions, &DataType::Utf8).map_err(|error| error.to_string())?;
        let descriptions = descriptions
            .as_string::<i32>()
            .iter()
            .map(|description| description.map(str::to_owned))
            .collect();
        Ok(Self(Some(descriptions)))
    }
}

/// How a judgment's answers are stored: the type of each subtask's value,
/// as Spiral's function typed it, and one episode's values.
pub(super) trait StoredJudgment: Judgment {
    fn value_type() -> DataType;
    /// Each subtask's value, NULL where jev did not answer it.
    fn values(answers: Vec<Option<Self::Answer>>) -> Result<ArrayRef, String>;
}

impl StoredJudgment for SimilarityQuestions {
    fn value_type() -> DataType {
        DataType::Float64
    }

    fn values(answers: Vec<Option<f64>>) -> Result<ArrayRef, String> {
        Ok(Arc::new(Float64Array::from(answers)))
    }
}

impl StoredJudgment for CategoryQuestions {
    fn value_type() -> DataType {
        DataType::Struct(Self::fields())
    }

    fn values(answers: Vec<Option<(String, f64)>>) -> Result<ArrayRef, String> {
        let valid = NullBuffer::from_iter(answers.iter().map(Option::is_some));
        let (categories, probabilities): (Vec<_>, Vec<_>) =
            answers.into_iter().map(Option::unwrap_or_default).unzip();
        let values = StructArray::try_new(
            Self::fields(),
            vec![
                Arc::new(StringArray::from(categories)),
                Arc::new(Float64Array::from(probabilities)),
            ],
            Some(valid),
        )
        .map_err(|error| error.to_string())?;
        Ok(Arc::new(values))
    }
}

impl CategoryQuestions {
    fn fields() -> Fields {
        Fields::from(vec![
            Field::new("category", DataType::Utf8, false),
            Field::new("probability", DataType::Float64, false),
        ])
    }
}

/// One judgment of every subtask of an episode, asked of `model` through
/// OpenRouter.
pub(super) struct SubtaskJudgment<'a, J> {
    questions: J,
    model: &'a str,
    openrouter: &'a OpenRouter,
}

impl<'a, J: StoredJudgment> SubtaskJudgment<'a, J> {
    pub(super) fn new(questions: J, model: &'a str, openrouter: &'a OpenRouter) -> Self {
        Self {
            questions,
            model,
            openrouter,
        }
    }

    /// The type of one episode's answer: a list of its subtasks' values.
    fn answer_type() -> DataType {
        DataType::new_list(J::value_type(), true)
    }

    /// `episode`'s answer: the judgment of each of its `stored` subtasks. An
    /// `Err` is the run's.
    pub(super) async fn answer(
        &self,
        episode: &EpisodeId,
        stored: StoredSubtasks,
    ) -> Result<Answered, String> {
        let Some(descriptions) = stored.0 else {
            return Ok(Answered::failed(
                &Self::answer_type(),
                "Break down stored no subtasks for this episode.",
            ));
        };
        let rows = descriptions
            .iter()
            .enumerate()
            .filter_map(|(row, description)| Some((row, description.as_deref()?)))
            .collect::<Vec<_>>();
        let batches = self.questions.batches(&rows);
        let answered = join_all(batches.iter().map(|batch| async {
            let answer = self
                .openrouter
                .decisions(&batch.request(self.model))
                .await?;
            DecisionAnswers::parse(answer).map_err(ProviderFailure::Request)
        }))
        .await;
        let mut values = descriptions.iter().map(|_| None).collect::<Vec<_>>();
        for (batch, answers) in batches.iter().zip(answered) {
            let answers = match answers {
                Ok(answers) => answers,
                Err(ProviderFailure::Account(reason)) => return Err(reason),
                Err(ProviderFailure::Request(reason)) => {
                    for &row in batch.rows() {
                        Self::unanswered(episode, row, &reason);
                    }
                    continue;
                }
            };
            for &row in batch.rows() {
                match self.questions.read(&answers, row) {
                    Ok(answer) => values[row] = Some(answer),
                    Err(reason) => Self::unanswered(episode, row, &reason),
                }
            }
        }
        Ok(Answered::given(EpisodeAnswer::listing(J::values(values)?)))
    }

    /// A subtask jev did not answer is NULL in the list, and its reason is
    /// logged.
    fn unanswered(episode: &EpisodeId, subtask: usize, reason: &str) {
        tracing::warn!(%episode, subtask, reason, "jev did not judge a subtask; its judgment is NULL");
    }
}

#[cfg(test)]
mod tests;
