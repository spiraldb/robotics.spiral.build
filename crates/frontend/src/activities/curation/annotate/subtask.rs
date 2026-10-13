//! One subtask of one episode, as a preview holds it: the interval Break down
//! described, and what the Find similar or Group similar node that read it
//! judged its description to be.
use crate::activities::curation::document::SubtaskKey;
use crate::activities::curation::population::EpisodeSample;
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::relations::SpiralQuerySnapshot;

/// The most subtasks a preview reads for each episode it lists: the query
/// endpoint's own bound. Break down answers a few dozen for a long episode.
pub(in crate::activities::curation) const MAX_SUBTASKS: usize = 1_000;

/// The category a grouping put one subtask in, and how probable that was.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct CategoryPlacement {
    category: String,
    probability: f64,
}

impl CategoryPlacement {
    pub(in crate::activities::curation) fn new(
        category: impl Into<String>,
        probability: f64,
    ) -> Result<Self, String> {
        let placement = Self {
            category: category.into(),
            probability,
        };
        if placement.category.trim().is_empty() || !(0.0..=1.0).contains(&placement.probability) {
            return Err("A placement needs its category and a probability in [0, 1].".into());
        }
        Ok(placement)
    }
    pub(in crate::activities::curation) fn category(&self) -> &str {
        &self.category
    }
    pub(in crate::activities::curation) fn probability(&self) -> f64 {
        self.probability
    }
}

/// What a node judged a subtask to be. A judgment the model could not give
/// is `None`.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum JudgmentValue {
    /// How likely the subtask is the operation a search asked about.
    Similarity(Option<f64>),
    /// The category a grouping put the subtask in.
    Category(Option<CategoryPlacement>),
}

/// One subtask interval of one episode. `start_time` is inclusive and
/// `end_time` exclusive, in seconds from the episode's first message.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct SubtaskRow {
    episode: EpisodeId,
    start_time: f64,
    end_time: f64,
    description: String,
    judgment: Option<JudgmentValue>,
}

impl SubtaskRow {
    pub(in crate::activities::curation) fn new(
        episode: EpisodeId,
        (start_time, end_time): (f64, f64),
        description: impl Into<String>,
    ) -> Result<Self, String> {
        let row = Self {
            episode,
            start_time,
            end_time,
            description: description.into().trim().to_owned(),
            judgment: None,
        };
        if !start_time.is_finite()
            || !end_time.is_finite()
            || start_time < 0.0
            || end_time <= start_time
        {
            return Err(format!(
                "Subtask interval [{start_time}, {end_time}) is not a valid time range."
            ));
        }
        if row.description.is_empty() {
            return Err("A subtask description is empty.".into());
        }
        Ok(row)
    }
    /// The same subtask, judged by the node that read it.
    pub(in crate::activities::curation) fn judged(mut self, value: JudgmentValue) -> Self {
        self.judgment = Some(value);
        self
    }
    pub(in crate::activities::curation) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(in crate::activities::curation) fn start_time(&self) -> f64 {
        self.start_time
    }
    pub(in crate::activities::curation) fn end_time(&self) -> f64 {
        self.end_time
    }
    pub(in crate::activities::curation) fn description(&self) -> &str {
        &self.description
    }
    /// Where in the interval its thumbnail is taken. The middle shows the
    /// action under way rather than the reach into it or the release out of
    /// it, which is what tells two similar subtasks apart.
    pub(in crate::activities::curation) fn midpoint(&self) -> f64 {
        f64::midpoint(self.start_time, self.end_time)
    }
    /// How likely the search that read it found it the operation it asked
    /// about.
    pub(in crate::activities::curation) fn similarity(&self) -> Option<f64> {
        match &self.judgment {
            Some(JudgmentValue::Similarity(similarity)) => *similarity,
            _ => None,
        }
    }
    /// Where the grouping that read it put it.
    pub(in crate::activities::curation) fn placement(&self) -> Option<&CategoryPlacement> {
        match &self.judgment {
            Some(JudgmentValue::Category(placement)) => placement.as_ref(),
            _ => None,
        }
    }
    pub(in crate::activities::curation) fn category(&self) -> Option<&str> {
        self.placement().map(CategoryPlacement::category)
    }
    /// Whether this is the row a focus or a remembered question refers to.
    pub(in crate::activities::curation) fn is(&self, episode: &EpisodeId, start_time: f64) -> bool {
        &self.episode == episode && self.start_time == start_time
    }
    pub(in crate::activities::curation) fn key(&self) -> SubtaskKey {
        SubtaskKey::new(self.episode.clone(), self.start_time)
    }
}

/// What a node's preview answers of each subtask beside its interval and
/// description: nothing more for Break down, a similarity for Find similar,
/// and a category with its probability for Group similar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) enum SubtaskReading {
    Described,
    Similarity,
    Category,
}

impl SubtaskReading {
    /// The subtasks of the rows Spiral answered with, each episode's in time
    /// order. The relation's times are the recording's clock; the browser
    /// reads them from each episode's first message, as `sample` lists it.
    pub(in crate::activities::curation) fn read(
        self,
        answer: &SpiralQuerySnapshot,
        sample: &EpisodeSample,
    ) -> Result<Vec<SubtaskRow>, String> {
        if answer.truncated() {
            return Err("The preview answered with more subtasks than one read holds.".into());
        }
        let mut rows = answer
            .rows()
            .iter()
            .map(|row| self.read_row(row, sample))
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_by(|left, right| {
            left.episode
                .cmp(&right.episode)
                .then(left.start_time.total_cmp(&right.start_time))
        });
        Ok(rows)
    }

    fn read_row(
        self,
        row: &Map<String, Value>,
        sample: &EpisodeSample,
    ) -> Result<SubtaskRow, String> {
        let episode = row
            .get("episode_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "A subtask of the preview names no episode.".to_owned())
            .and_then(EpisodeId::try_from)?;
        let origin = sample
            .origin_ns(&episode)
            .ok_or("A subtask of the preview belongs to an episode the sample does not list.")?;
        let seconds = |column: &str| {
            row.get(column)
                .and_then(Value::as_i64)
                .map(|nanos| nanos.saturating_sub(origin) as f64 / 1e9)
                .ok_or_else(|| format!("A subtask of the preview has no {column}."))
        };
        let described = SubtaskRow::new(
            episode,
            (seconds("start_ns")?, seconds("end_ns")?),
            row.get("description")
                .and_then(Value::as_str)
                .ok_or("A subtask of the preview has no description.")?,
        )?;
        Ok(match self {
            Self::Described => described,
            Self::Similarity => described.judged(JudgmentValue::Similarity(
                row.get("similarity").and_then(Value::as_f64),
            )),
            Self::Category => {
                let placement = match row.get("category").and_then(Value::as_str) {
                    None => None,
                    Some(category) => Some(CategoryPlacement::new(
                        category,
                        row.get("probability")
                            .and_then(Value::as_f64)
                            .ok_or("A category of the preview has no probability.")?,
                    )?),
                };
                described.judged(JudgmentValue::Category(placement))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::episode::Episode;
    use serde_json::json;

    /// One episode whose first message is at one second of log time.
    fn sample() -> EpisodeSample {
        EpisodeSample::fixture(vec![Episode::fixture(
            &EpisodeId::fixture(1),
            10_000_000_000,
        )])
    }

    fn answer(rows: Vec<Value>) -> SpiralQuerySnapshot {
        SpiralQuerySnapshot::new(
            Vec::new(),
            rows.into_iter()
                .map(|row| match row {
                    Value::Object(row) => row,
                    _ => panic!("a row is an object"),
                })
                .collect(),
            false,
        )
    }

    #[test]
    fn an_answer_reads_back_as_subtasks_with_the_judgment_the_node_made() {
        let id = EpisodeId::fixture(1).to_string();
        let start = 1_000_000_000_i64;
        let rows = SubtaskReading::Category
            .read(
                &answer(vec![
                    json!({"episode_id": id, "subtask_index": 1, "start_ns": start + 4_000_000_000_i64, "end_ns": start + 9_000_000_000_i64,
                           "description": "fold the skirt", "category": "fold", "probability": 0.8}),
                    json!({"episode_id": id, "subtask_index": 0, "start_ns": start, "end_ns": start + 4_000_000_000_i64,
                           "description": " grasp the hem ", "category": null, "probability": null}),
                ]),
                &sample(),
            )
            .unwrap();
        assert_eq!((rows[0].start_time(), rows[0].end_time()), (0.0, 4.0));
        assert_eq!(rows[0].description(), "grasp the hem");
        assert_eq!(rows[0].category(), None, "a row the model could not place");
        assert_eq!(rows[1].category(), Some("fold"));
        let similar = SubtaskReading::Similarity
            .read(
                &answer(vec![json!({"episode_id": id, "subtask_index": 0, "start_ns": start, "end_ns": start + 1_000_000_000_i64,
                                    "description": "reach", "similarity": 0.9})]),
                &sample(),
            )
            .unwrap();
        assert_eq!(similar[0].similarity(), Some(0.9));
        assert!(
            SubtaskReading::Described
                .read(&answer(vec![json!({"episode_id": EpisodeId::fixture(2).to_string(), "subtask_index": 0, "start_ns": 0, "end_ns": 1, "description": "x"})]), &sample())
                .unwrap_err()
                .contains("does not list")
        );
    }
}
