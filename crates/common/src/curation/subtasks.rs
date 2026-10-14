//! What Break down and the judgments of its subtasks ask a model, as a fill
//! run carries it to the backend, which reads what each call names of the
//! episode, asks OpenRouter, and stores the answer.
//!
//! Break down splits one episode's camera video into consecutive subtasks
//! with a multimodal model. Find similar asks a decision model how likely
//! each subtask a Break down column stores is the operation a query names,
//! and Group similar which of its categories each one belongs to; both
//! answer with one list per episode, aligned with its subtasks.
use super::answers::AnswerColumn;
use crate::datasets::EpisodeTask;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// How far apart Break down samples an episode's pictures, to the
/// millisecond.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SampleInterval {
    milliseconds: NonZeroU32,
}

impl SampleInterval {
    /// `seconds` apart: a positive, whole number of milliseconds.
    pub fn from_seconds(seconds: f64) -> Result<Self, String> {
        let milliseconds = (seconds * 1000.0).round();
        let whole = (milliseconds / 1000.0 - seconds).abs() < 1e-9;
        if !whole || !(1.0..=f64::from(u32::MAX)).contains(&milliseconds) {
            return Err(
                "The sample interval must be a positive number of seconds, to the millisecond."
                    .into(),
            );
        }
        let milliseconds = NonZeroU32::new(milliseconds as u32)
            .ok_or("The sample interval must be at least a millisecond.")?;
        Ok(Self { milliseconds })
    }

    pub fn seconds(self) -> f64 {
        f64::from(self.milliseconds.get()) / 1000.0
    }
}

/// Break down's call: the subtasks of each episode's video on the camera
/// topic `camera`, sampled every `sample_interval`, as `model` reads them,
/// told the episode's task when its dataset records one where `task` says.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskBreakdown {
    camera: String,
    sample_interval: SampleInterval,
    model: String,
    task: Option<EpisodeTask>,
}

impl SubtaskBreakdown {
    pub fn new(
        camera: impl Into<String>,
        sample_interval: SampleInterval,
        model: impl Into<String>,
        task: Option<EpisodeTask>,
    ) -> Self {
        Self {
            camera: camera.into(),
            sample_interval,
            model: model.into(),
            task,
        }
    }
    pub fn camera(&self) -> &str {
        &self.camera
    }
    pub fn sample_interval(&self) -> SampleInterval {
        self.sample_interval
    }
    /// The OpenRouter id of the multimodal model asked.
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn task(&self) -> Option<&EpisodeTask> {
        self.task.as_ref()
    }
}

/// Find similar's call: how likely each subtask the Break down column
/// `subtasks` stores describes the operation `query` names, as `model`
/// decides.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskSimilarity {
    subtasks: AnswerColumn,
    query: String,
    model: String,
}

impl SubtaskSimilarity {
    pub fn new(subtasks: AnswerColumn, query: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            subtasks,
            query: query.into(),
            model: model.into(),
        }
    }
    pub fn subtasks(&self) -> &AnswerColumn {
        &self.subtasks
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    /// The OpenRouter id of the decision model asked.
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// One category Group similar may put a subtask in: its name, which the
/// answer names it by, and the definition that tells it apart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryDefinition {
    name: String,
    definition: String,
}

impl CategoryDefinition {
    pub fn new(name: impl Into<String>, definition: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn definition(&self) -> &str {
        &self.definition
    }
}

/// Group similar's call: which of `categories` each subtask the Break down
/// column `subtasks` stores fits best, and how probable that is, as `model`
/// decides.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskClassification {
    subtasks: AnswerColumn,
    categories: Vec<CategoryDefinition>,
    model: String,
}

impl SubtaskClassification {
    pub fn new(
        subtasks: AnswerColumn,
        categories: Vec<CategoryDefinition>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            subtasks,
            categories,
            model: model.into(),
        }
    }
    pub fn subtasks(&self) -> &AnswerColumn {
        &self.subtasks
    }
    pub fn categories(&self) -> &[CategoryDefinition] {
        &self.categories
    }
    /// The OpenRouter id of the decision model asked.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_interval_is_a_whole_positive_number_of_milliseconds() {
        assert_eq!(SampleInterval::from_seconds(0.5).unwrap().seconds(), 0.5);
        assert_eq!(SampleInterval::from_seconds(0.3).unwrap().seconds(), 0.3);
        assert_eq!(
            serde_json::to_value(SampleInterval::from_seconds(1.25).unwrap()).unwrap(),
            serde_json::json!(1250)
        );
        for refused in [0.0, -1.0, 0.0004, 0.1234, f64::NAN, f64::INFINITY] {
            assert!(SampleInterval::from_seconds(refused).is_err(), "{refused}");
        }
        assert!(serde_json::from_value::<SampleInterval>(serde_json::json!(0)).is_err());
    }
}
