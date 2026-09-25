//! One subtask of one episode, as a preview holds it: the interval Break down
//! described, and what every Find similar or Group similar node on the way
//! judged its description to be.
//!
//! A preview reads its subtasks back from the rows Spiral answers with, and
//! writes them into the next node's preview as `VALUES`, so both directions
//! of one relation of subtasks live here ([`SubtaskColumns`]).
use crate::activities::curation::document::SubtaskKey;
use crate::activities::curation::path::Relation;
use crate::activities::curation::population::EpisodeSample;
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::relations::SpiralQuerySnapshot;
use spiral_robotics_common::sql::SqlLiteral;

/// The most subtasks one preview query answers with: the query endpoint's own
/// bound. Break down answers a few dozen for a long episode.
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

/// What one node judged a subtask to be. A judgment the model could not give
/// is `None`.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum JudgmentValue {
    /// How likely the subtask is the operation a search asked about.
    Similarity(Option<f64>),
    /// The category a grouping put the subtask in.
    Category(Option<CategoryPlacement>),
}

/// One judgment, under the column its node wrote it to.
#[derive(Clone, Debug, PartialEq)]
struct Judgment {
    column: String,
    value: JudgmentValue,
}

/// One subtask interval of one episode. `start_time` is inclusive and
/// `end_time` exclusive, in seconds from the episode's first message.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct SubtaskRow {
    episode: EpisodeId,
    start_time: f64,
    end_time: f64,
    description: String,
    /// In path order, so the last of a kind is the nearest node's.
    judgments: Vec<Judgment>,
}

impl SubtaskRow {
    /// The column a subtask's description travels in, on every edge below
    /// Break down.
    pub(in crate::activities::curation) const DESCRIPTION: &'static str = "subtask";
    /// The measure Find similar writes.
    pub(in crate::activities::curation) const SIMILARITY: &'static str = "similarity";
    /// The measure Group similar writes: the category and its probability.
    pub(in crate::activities::curation) const CATEGORY: &'static str = "subtask_category";

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
            judgments: Vec::new(),
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
    /// The same subtask with one more judgment, written to `column`.
    pub(in crate::activities::curation) fn judged(
        mut self,
        column: impl Into<String>,
        value: JudgmentValue,
    ) -> Self {
        self.judgments.push(Judgment {
            column: column.into(),
            value,
        });
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
    /// Where in the interval its still is cut from. The middle shows the
    /// action under way rather than the reach into it or the release out of
    /// it, which is what tells two similar subtasks apart.
    pub(in crate::activities::curation) fn midpoint(&self) -> f64 {
        f64::midpoint(self.start_time, self.end_time)
    }
    /// How likely the nearest search found this subtask the operation it
    /// asked about.
    pub(in crate::activities::curation) fn similarity(&self) -> Option<f64> {
        self.judgments
            .iter()
            .rev()
            .find_map(|judgment| match &judgment.value {
                JudgmentValue::Similarity(similarity) => Some(*similarity),
                JudgmentValue::Category(_) => None,
            })
            .flatten()
    }
    /// Where the nearest grouping put this subtask.
    pub(in crate::activities::curation) fn placement(&self) -> Option<&CategoryPlacement> {
        self.judgments
            .iter()
            .rev()
            .find_map(|judgment| match &judgment.value {
                JudgmentValue::Category(placement) => Some(placement.as_ref()),
                JudgmentValue::Similarity(_) => None,
            })
            .flatten()
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
    fn judgment(&self, column: &str) -> Option<&JudgmentValue> {
        self.judgments
            .iter()
            .find(|judgment| judgment.column == column)
            .map(|judgment| &judgment.value)
    }
}

/// What one column of a relation of subtasks holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SubtaskColumn {
    Description,
    Similarity,
    Category,
}

/// The columns of one relation of subtasks after its interval, in order, as
/// the path names them: the description, then every judgment made on the way.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct SubtaskColumns(Vec<(String, SubtaskColumn)>);

impl SubtaskColumns {
    /// The columns `relation` carries; a relation of anything but subtasks
    /// has none of these.
    pub(in crate::activities::curation) fn of(relation: &Relation) -> Result<Self, String> {
        relation
            .properties()
            .map(|(column, measure)| {
                let kind = match measure {
                    SubtaskRow::DESCRIPTION => SubtaskColumn::Description,
                    SubtaskRow::SIMILARITY => SubtaskColumn::Similarity,
                    SubtaskRow::CATEGORY => SubtaskColumn::Category,
                    other => return Err(format!("A relation of subtasks carries no {other}.")),
                };
                Ok((column.to_owned(), kind))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// The subtasks of the rows Spiral answered with, each episode's in time
    /// order. The relation's times are the recording's clock; the browser
    /// reads them from each episode's first message, as `sample` lists it.
    pub(in crate::activities::curation) fn read(
        &self,
        answer: &SpiralQuerySnapshot,
        sample: &EpisodeSample,
    ) -> Result<Vec<SubtaskRow>, String> {
        if answer.truncated() {
            return Err(format!(
                "The preview answered with more than {MAX_SUBTASKS} subtasks."
            ));
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
        &self,
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
        let (start, end) = (seconds("start_ns")?, seconds("end_ns")?);
        let mut described = None::<SubtaskRow>;
        for (column, kind) in &self.0 {
            let cell = row.get(column).unwrap_or(&Value::Null);
            described = Some(match (kind, described) {
                (SubtaskColumn::Description, None) => SubtaskRow::new(
                    episode.clone(),
                    (start, end),
                    cell.as_str()
                        .ok_or("A subtask of the preview has no description.")?,
                )?,
                (SubtaskColumn::Similarity, Some(row)) => {
                    row.judged(column.clone(), JudgmentValue::Similarity(cell.as_f64()))
                }
                (SubtaskColumn::Category, Some(row)) => {
                    let placement = match cell {
                        Value::Null => None,
                        placed => Some(CategoryPlacement::new(
                            placed
                                .get("category")
                                .and_then(Value::as_str)
                                .ok_or("A category of the preview has no name.")?,
                            placed
                                .get("probability")
                                .and_then(Value::as_f64)
                                .ok_or("A category of the preview has no probability.")?,
                        )?),
                    };
                    row.judged(column.clone(), JudgmentValue::Category(placement))
                }
                _ => return Err("A relation of subtasks starts with their description.".into()),
            });
        }
        described.ok_or_else(|| "The preview names no subtask columns.".into())
    }

    /// The names the `VALUES` of these subtasks list their columns under: a
    /// category travels as its name and probability, and is put back
    /// together by [`Self::rebuilt`], since `VALUES` takes literals only.
    pub(in crate::activities::curation) fn value_names(&self) -> Vec<String> {
        let mut names = vec![
            "episode_id".to_owned(),
            "start_ns".to_owned(),
            "end_ns".to_owned(),
        ];
        for (column, kind) in &self.0 {
            match kind {
                SubtaskColumn::Description | SubtaskColumn::Similarity => {
                    names.push(column.clone())
                }
                SubtaskColumn::Category => {
                    names.push(format!("{column}_category"));
                    names.push(format!("{column}_probability"));
                }
            }
        }
        names
    }

    /// Every column of the relation, made from the `VALUES` columns.
    pub(in crate::activities::curation) fn rebuilt(&self) -> Vec<String> {
        let mut columns = vec![
            "episode_id".to_owned(),
            "start_ns".to_owned(),
            "end_ns".to_owned(),
        ];
        for (column, kind) in &self.0 {
            columns.push(match kind {
                SubtaskColumn::Description | SubtaskColumn::Similarity => column.clone(),
                SubtaskColumn::Category => format!(
                    "CASE WHEN {column}_category IS NULL THEN NULL \
                     ELSE STRUCT_PACK(category := {column}_category, probability := {column}_probability) END AS {column}"
                ),
            });
        }
        columns
    }

    /// One subtask as a `VALUES` row, its episode spelled as text: a judgment
    /// reads the id through and never compares it with the dataset's.
    pub(in crate::activities::curation) fn values(
        &self,
        row: &SubtaskRow,
        origin_ns: i64,
    ) -> Result<String, String> {
        let nanos = |seconds: f64| {
            format!(
                "CAST({} AS BIGINT)",
                origin_ns + (seconds * 1e9).round() as i64
            )
        };
        let unit = |value: f64| {
            (0.0..=1.0)
                .contains(&value)
                .then(|| SqlLiteral::double(value))
                .flatten()
                .ok_or_else(|| format!("A judgment of {value} is not between 0 and 1."))
        };
        let mut values = vec![
            SqlLiteral::text(row.episode.as_str()),
            nanos(row.start_time),
            nanos(row.end_time),
        ];
        for (column, kind) in &self.0 {
            match (kind, row.judgment(column)) {
                (SubtaskColumn::Description, _) => values.push(SqlLiteral::text(&row.description)),
                (SubtaskColumn::Similarity, Some(JudgmentValue::Similarity(similarity))) => {
                    values.push(match similarity {
                        Some(similarity) => unit(*similarity)?,
                        None => "CAST(NULL AS DOUBLE)".to_owned(),
                    });
                }
                (SubtaskColumn::Category, Some(JudgmentValue::Category(placement))) => {
                    match placement {
                        Some(placement) => {
                            values.push(SqlLiteral::text(placement.category()));
                            values.push(unit(placement.probability())?);
                        }
                        None => {
                            values.push("CAST(NULL AS VARCHAR)".to_owned());
                            values.push("CAST(NULL AS DOUBLE)".to_owned());
                        }
                    }
                }
                _ => return Err(format!("A subtask has no {column} to hand on.")),
            }
        }
        Ok(format!("({})", values.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::episode::Episode;
    use serde_json::json;

    fn columns() -> SubtaskColumns {
        SubtaskColumns::of(
            &Relation::new("breakdown", vec![SubtaskRow::DESCRIPTION])
                .measuring("grouping", SubtaskRow::CATEGORY)
                .measuring("search", SubtaskRow::SIMILARITY),
        )
        .unwrap()
    }

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
    fn an_answer_reads_back_as_subtasks_with_every_judgment_on_the_way() {
        let id = EpisodeId::fixture(1).to_string();
        let start = 1_000_000_000_i64;
        let rows = columns()
            .read(
                &answer(vec![
                    json!({"episode_id": id, "start_ns": start + 4_000_000_000_i64, "end_ns": start + 9_000_000_000_i64,
                           "subtask": "fold the skirt", "subtask_category": {"category": "fold", "probability": 0.8}, "similarity": 0.9}),
                    json!({"episode_id": id, "start_ns": start, "end_ns": start + 4_000_000_000_i64,
                           "subtask": " grasp the hem ", "subtask_category": null, "similarity": null}),
                ]),
                &sample(),
            )
            .unwrap();
        assert_eq!((rows[0].start_time(), rows[0].end_time()), (0.0, 4.0));
        assert_eq!(rows[0].description(), "grasp the hem");
        assert_eq!(rows[0].category(), None, "a row the model could not place");
        assert_eq!(rows[1].category(), Some("fold"));
        assert_eq!(rows[1].similarity(), Some(0.9));
        assert!(
            columns()
                .read(&answer(vec![json!({"episode_id": EpisodeId::fixture(2).to_string(), "start_ns": 0, "end_ns": 1, "subtask": "x"})]), &sample())
                .unwrap_err()
                .contains("does not list")
        );
    }

    #[test]
    fn a_subtask_is_handed_on_as_literals_and_put_back_together() {
        let row = SubtaskRow::new(EpisodeId::fixture(1), (0.5, 4.0), "fold the towel's edge")
            .unwrap()
            .judged(
                "subtask_category",
                JudgmentValue::Category(Some(CategoryPlacement::new("fold", 0.75).unwrap())),
            )
            .judged("similarity", JudgmentValue::Similarity(None));
        assert_eq!(
            columns().values(&row, 1_000_000_000).unwrap(),
            format!(
                "('{}', CAST(1500000000 AS BIGINT), CAST(5000000000 AS BIGINT), 'fold the towel''s edge', 'fold', CAST(0.75 AS DOUBLE), CAST(NULL AS DOUBLE))",
                EpisodeId::fixture(1)
            )
        );
        assert_eq!(
            columns().value_names(),
            [
                "episode_id",
                "start_ns",
                "end_ns",
                "subtask",
                "subtask_category_category",
                "subtask_category_probability",
                "similarity"
            ]
        );
        assert_eq!(
            columns().rebuilt()[4],
            "CASE WHEN subtask_category_category IS NULL THEN NULL ELSE STRUCT_PACK(category := subtask_category_category, probability := subtask_category_probability) END AS subtask_category"
        );
        // A subtask read before a judgment was made has none to hand on.
        let described = SubtaskRow::new(EpisodeId::fixture(1), (0.0, 1.0), "grasp").unwrap();
        assert!(columns().values(&described, 0).is_err());
    }
}
