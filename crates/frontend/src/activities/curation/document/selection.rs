//! What a node keeps of what it measured, or a Select of what reaches it: the
//! node's rule.
//!
//! A rule narrows; it never measures. What it keeps by is its basis: a Score
//! progress node's progress rate, a Track objects node's count, a Find
//! similar node's similarity, and a Group similar node's categories are those
//! nodes' own knobs, and a Select
//! keeps by the dataset's recorded facts of each episode, as many conditions
//! as it names — never by what a model answered. The rule is stored in those
//! terms, so it reads as one sentence about the node it belongs to.
//!
//! One rule has two readings that must agree: the browser keeps what the
//! preview read as the threshold slides (`keeps_*`), and the path compiles it
//! to the `WHERE` the published view keeps by (`predicate`).
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::path::{PathError, PathFragment, PathStep, Relation, StepSql};
use crate::activities::curation::score_progress::Moment;
use crate::activities::curation::track_objects::ObjectCount;
use crate::activities::episode::Episode;
use crate::format::{ClockPresentation as _, GroupedCountPresentation as _};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::sql::SqlLiteral;

/// What a rule keeps by, and so what it can name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionBasis {
    /// The dataset's own statistics of each episode: a Select's.
    EpisodeFacts,
    /// A Find similar node's probability for each subtask.
    Similarity,
    /// A Group similar node's category for each subtask.
    Categories,
    /// A Score progress node's progress rate for each moment.
    Progress,
    /// A Track objects node's count for each episode: the most it saw at
    /// once.
    ObjectCount,
}

/// One statistic Spiral records for every episode.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EpisodeFact {
    DurationSeconds,
    Messages,
    Channels,
    SizeMegabytes,
}

impl EpisodeFact {
    pub(crate) const ALL: [Self; 4] = [
        Self::DurationSeconds,
        Self::Messages,
        Self::Channels,
        Self::SizeMegabytes,
    ];
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration_seconds",
            Self::Messages => "messages",
            Self::Channels => "channels",
            Self::SizeMegabytes => "size_megabytes",
        }
    }
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|fact| fact.key() == key)
    }
    /// The fact as a condition's row starts with it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DurationSeconds => "Duration",
            Self::Messages => "Message count",
            Self::Channels => "Channel count",
            Self::SizeMegabytes => "File size",
        }
    }
    /// The fact as the noun of "Keep episodes whose … is at least".
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration",
            Self::Messages => "message count",
            Self::Channels => "channel count",
            Self::SizeMegabytes => "file size",
        }
    }
    /// What the threshold is counted in, when the fact has a unit.
    pub(crate) const fn unit(self) -> Option<&'static str> {
        match self {
            Self::DurationSeconds => Some("s"),
            Self::SizeMegabytes => Some("MB"),
            Self::Messages | Self::Channels => None,
        }
    }
    /// The fact's value for one episode as a row reads it, in its own unit:
    /// "0:20.5", "11,404", "3", "15.2 MB".
    pub(in crate::activities::curation) fn reading(self, episode: &Episode) -> Option<String> {
        let value = self.of(episode)?;
        Some(match self {
            Self::DurationSeconds => value.clock_label(),
            Self::Messages | Self::Channels => (value as u64).grouped_label(),
            Self::SizeMegabytes => format!("{value:.1} MB"),
        })
    }
    /// The fact as a column of an episode listing, the same value `of` reads
    /// in the browser.
    pub(in crate::activities::curation) const fn expression(self) -> &'static str {
        match self {
            Self::DurationSeconds => "CAST(duration_ns AS DOUBLE) / 1e9",
            Self::Messages => "message_count",
            Self::Channels => "channel_count",
            Self::SizeMegabytes => "CAST(source_bytes AS DOUBLE) / 1e6",
        }
    }
    /// The fact's value for one episode, when Spiral listed it.
    pub(in crate::activities::curation) fn of(self, episode: &Episode) -> Option<f64> {
        let facts = episode.facts();
        match self {
            Self::DurationSeconds => facts.duration_seconds(),
            Self::Messages => facts.message_count().map(|count| count as f64),
            Self::Channels => facts.channel_count().map(|count| count as f64),
            Self::SizeMegabytes => facts.source_bytes().map(|bytes| bytes as f64 / 1e6),
        }
    }
}

/// Which side of its value a condition keeps.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Comparison {
    AtLeast,
    AtMost,
}

impl Comparison {
    pub(crate) const ALL: [Self; 2] = [Self::AtLeast, Self::AtMost];
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::AtLeast => "at_least",
            Self::AtMost => "at_most",
        }
    }
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|comparison| comparison.key() == key)
    }
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::AtLeast => "at least",
            Self::AtMost => "at most",
        }
    }
    fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::AtLeast => value >= threshold,
            Self::AtMost => value <= threshold,
        }
    }
    /// The comparison as SQL spells it, as `holds` reads it.
    const fn operator(self) -> &'static str {
        match self {
            Self::AtLeast => ">=",
            Self::AtMost => "<=",
        }
    }
}

/// One condition an episode meets: its recorded fact compared with a value.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpisodeCondition {
    fact: EpisodeFact,
    comparison: Comparison,
    value: f64,
}

impl EpisodeCondition {
    pub(crate) const fn new(fact: EpisodeFact, comparison: Comparison, value: f64) -> Self {
        Self {
            fact,
            comparison,
            value,
        }
    }
    pub(crate) const fn fact(&self) -> EpisodeFact {
        self.fact
    }
    pub(crate) const fn comparison(&self) -> Comparison {
        self.comparison
    }
    pub(crate) const fn value(&self) -> f64 {
        self.value
    }
    /// The same condition on another fact, from where it keeps everything.
    pub(crate) const fn on(self, fact: EpisodeFact) -> Self {
        Self::new(fact, self.comparison, 0.0)
    }
    pub(crate) const fn comparing(self, comparison: Comparison) -> Self {
        Self::new(self.fact, comparison, self.value)
    }
    pub(crate) const fn at(self, value: f64) -> Self {
        Self::new(self.fact, self.comparison, value)
    }
    /// Whether an episode meets it, by its recorded facts. An episode Spiral
    /// did not record the fact of does not.
    pub(in crate::activities::curation) fn holds(&self, episode: &Episode) -> bool {
        self.fact
            .of(episode)
            .is_some_and(|measured| self.comparison.holds(measured, self.value))
    }
    /// "duration at least 20 s".
    fn sentence(&self) -> String {
        let unit = self
            .fact
            .unit()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        format!(
            "{} {} {}{unit}",
            self.fact.label(),
            self.comparison.label(),
            self.value
        )
    }
    fn predicate(&self) -> Result<String, PathError> {
        let value = SqlLiteral::double(self.value)
            .ok_or_else(|| PathError::new("A Select's threshold must be a number."))?;
        Ok(format!(
            "{} {} {value}",
            self.fact.key(),
            self.comparison.operator()
        ))
    }
}

/// One rule, in the terms of its basis.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "keep", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Selection {
    /// Episodes that meet every condition.
    Episodes { conditions: Vec<EpisodeCondition> },
    /// Subtasks at least this likely to be the operation a search asked about.
    Similar { minimum: f64 },
    /// Subtasks in any of these categories, which the grouping names.
    Categories { names: Vec<String> },
    /// Moments progressing at least this fast. A moment the scorer could not
    /// read is kept: an unread moment is not a slow one.
    Progressing { minimum: f64 },
    /// Episodes showing at least `fewest` of what a Track objects node
    /// tracks at once, and at most `most` when it names a most. An episode
    /// the tracker could not read is not kept: nothing says how many it
    /// shows.
    ObjectCount { fewest: u64, most: Option<u64> },
}

impl Selection {
    /// A new rule. Episode conditions start where they keep every episode, so
    /// adding a Select never silently drops data; categories start empty,
    /// because a grouping has none until they are proposed, and then keeps
    /// them all.
    pub(crate) fn for_basis(basis: SelectionBasis) -> Self {
        match basis {
            SelectionBasis::EpisodeFacts => Self::Episodes {
                conditions: vec![EpisodeCondition::new(
                    EpisodeFact::DurationSeconds,
                    Comparison::AtLeast,
                    0.0,
                )],
            },
            SelectionBasis::Similarity => Self::Similar { minimum: 0.5 },
            SelectionBasis::Categories => Self::Categories { names: Vec::new() },
            SelectionBasis::Progress => Self::Progressing { minimum: 0.0 },
            SelectionBasis::ObjectCount => Self::ObjectCount {
                fewest: 0,
                most: None,
            },
        }
    }
    pub(crate) fn basis(&self) -> SelectionBasis {
        match self {
            Self::Episodes { .. } => SelectionBasis::EpisodeFacts,
            Self::Similar { .. } => SelectionBasis::Similarity,
            Self::Categories { .. } => SelectionBasis::Categories,
            Self::Progressing { .. } => SelectionBasis::Progress,
            Self::ObjectCount { .. } => SelectionBasis::ObjectCount,
        }
    }
    /// Whether an episode is kept: it meets every condition.
    pub(in crate::activities::curation) fn keeps_episode(&self, episode: &Episode) -> bool {
        match self {
            Self::Episodes { conditions } => {
                conditions.iter().all(|condition| condition.holds(episode))
            }
            _ => false,
        }
    }
    /// Whether a subtask is kept, by its probability in a search or its
    /// category in a grouping. A subtask nothing has scored or placed yet is
    /// not kept: nothing says it belongs.
    pub(crate) fn keeps_subtask(&self, probability: Option<f64>, category: Option<&str>) -> bool {
        match self {
            Self::Similar { minimum } => probability.is_some_and(|value| value >= *minimum),
            Self::Categories { names } => {
                category.is_some_and(|category| names.iter().any(|name| name == category))
            }
            _ => false,
        }
    }
    /// Whether a moment is kept, by its progress rate.
    pub(crate) fn keeps_moment(&self, velocity: Option<f64>) -> bool {
        match self {
            Self::Progressing { minimum } => velocity.is_none_or(|value| value >= *minimum),
            _ => false,
        }
    }
    /// Whether an episode is kept, by the most a Track objects node saw at
    /// once in it.
    pub(crate) fn keeps_count(&self, count: Option<u64>) -> bool {
        match self {
            Self::ObjectCount { fewest, most } => {
                count.is_some_and(|count| count >= *fewest && most.is_none_or(|most| count <= most))
            }
            _ => false,
        }
    }
    /// The least a measure's rule keeps: the knob of a Score progress or a
    /// Find similar node. Episode conditions each have their own value, and
    /// categories none.
    pub(crate) fn minimum(&self) -> Option<f64> {
        match self {
            Self::Similar { minimum } | Self::Progressing { minimum } => Some(*minimum),
            Self::Episodes { .. } | Self::Categories { .. } | Self::ObjectCount { .. } => None,
        }
    }
    /// The same measure's rule keeping at least `value`; any other rule
    /// stays as it is.
    pub(crate) fn with_minimum(self, value: f64) -> Self {
        match self {
            Self::Similar { .. } => Self::Similar { minimum: value },
            Self::Progressing { .. } => Self::Progressing { minimum: value },
            rule => rule,
        }
    }
    /// The same count rule keeping from `fewest` to `most` objects, the
    /// larger taken as the most; any other rule stays as it is.
    pub(crate) fn with_count_range(self, (fewest, most): (u64, u64)) -> Self {
        match self {
            Self::ObjectCount { .. } => Self::ObjectCount {
                fewest: fewest.min(most),
                most: Some(fewest.max(most)),
            },
            rule => rule,
        }
    }
    /// A Select's conditions as its node card says them: "Duration at
    /// least 20 s and message count at most 5000".
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::Episodes { conditions } => {
                let mut sentence = conditions
                    .iter()
                    .map(EpisodeCondition::sentence)
                    .collect::<Vec<_>>()
                    .join(" and ");
                if let Some(first) = sentence.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                sentence
            }
            Self::Similar { minimum } => format!("At least {:.0}% similar", minimum * 100.0),
            Self::Categories { names } => names.join(", "),
            Self::Progressing { minimum } => format!("Progress rate at least {minimum:.2}×"),
            Self::ObjectCount { fewest, most } => match (*fewest, *most) {
                (0, None) => "Any number at once".to_owned(),
                (fewest, None) => format!("At least {fewest} at once"),
                (0, Some(most)) => format!("At most {most} at once"),
                (fewest, Some(most)) if fewest == most => format!("Exactly {fewest} at once"),
                (fewest, Some(most)) => format!("{fewest} to {most} at once"),
            },
        }
    }
    /// The rule as the predicate over the columns of `input`: what
    /// `keeps_*` keep, in SQL. A moment the scorer could not read is kept
    /// here too; a subtask the model could not judge is not.
    fn predicate(&self, input: &Relation) -> Result<String, PathError> {
        let literal = |value: f64| {
            SqlLiteral::double(value).ok_or_else(|| PathError::new("A threshold must be a number."))
        };
        let measured = |measure: &str| {
            input.measured(measure).ok_or_else(|| {
                PathError::new(format!("Nothing above this rule measures {measure}."))
            })
        };
        match self {
            Self::Episodes { conditions } => Ok(conditions
                .iter()
                .map(EpisodeCondition::predicate)
                .collect::<Result<Vec<_>, _>>()?
                .join(" AND ")),
            Self::Progressing { minimum } => Ok(format!(
                "({rate} IS NULL OR {rate} >= {})",
                literal(*minimum)?,
                rate = Moment::PROGRESS_RATE
            )),
            // An episode with no count compares as unknown, so it is not kept.
            Self::ObjectCount { fewest, most } => {
                let count = measured(ObjectCount::COLUMN)?;
                Ok(match most {
                    Some(most) => format!("{count} >= {fewest} AND {count} <= {most}"),
                    None => format!("{count} >= {fewest}"),
                })
            }
            Self::Similar { minimum } => Ok(format!(
                "{} >= {}",
                measured(SubtaskRow::SIMILARITY)?,
                literal(*minimum)?
            )),
            Self::Categories { names } if names.is_empty() => Err(PathError::new(
                "Keep at least one category: this grouping keeps none.",
            )),
            // The column is qualified by its relation, so the field is read
            // from the column rather than the column from a table.
            Self::Categories { names } => Ok(format!(
                "{}.{}.category IN ({})",
                input.name(),
                measured(SubtaskRow::CATEGORY)?,
                names
                    .iter()
                    .map(|name| SqlLiteral::text(name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Episodes { conditions } if conditions.is_empty() => {
                Err("A Select of episodes names at least one condition.".into())
            }
            Self::Episodes { conditions }
                if conditions
                    .iter()
                    .any(|condition| !condition.value.is_finite()) =>
            {
                Err("A Select's threshold must be a number.".into())
            }
            Self::Similar { minimum } if !(0.0..=1.0).contains(minimum) => {
                Err("A similarity threshold must be between 0% and 100%.".into())
            }
            Self::Progressing { minimum } if !minimum.is_finite() => {
                Err("A progress threshold must be a number.".into())
            }
            Self::ObjectCount {
                fewest,
                most: Some(most),
            } if most < fewest => Err("The most to keep cannot be fewer than the fewest.".into()),
            Self::Categories { names } => {
                let mut seen = std::collections::BTreeSet::new();
                for name in names {
                    if name.trim().is_empty() || !seen.insert(name.as_str()) {
                        return Err("A grouping keeps each category once.".into());
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// A rule of episodes keeps the rows that meet every condition, and a rule
/// of subtasks the ones it passes; a rule of moments keeps the moments that
/// pass and joins the ones that touch into stretches, each carrying its mean
/// progress rate.
impl PathFragment for Selection {
    fn interval_step(&self, step: &PathStep<'_>) -> Result<StepSql, PathError> {
        let input = step.input()?;
        let name = step.name();
        let predicate = self.predicate(input)?;
        let kept = format!(
            "{name} AS (SELECT {} FROM {} WHERE {predicate})",
            input.columns(),
            input.name()
        );
        Ok(match self {
            Self::Episodes { conditions } => StepSql::new(
                vec![kept],
                conditions
                    .iter()
                    .fold(input.renamed(name), |relation, condition| {
                        relation.publishing(condition.fact.key())
                    }),
            ),
            // Moments tile their episode, so kept moments with none left out
            // between them touch: numbering every moment and then the kept
            // ones, a stretch is where the two numbers keep one difference.
            // Each stretch is measured over a window rather than grouped, and
            // its first moment stands for it, so the answers riding on every
            // moment of an episode ride on with its stretches.
            Self::Progressing { .. } => {
                let (rate, columns) = (Moment::PROGRESS_RATE, input.columns());
                let stretch = "OVER (PARTITION BY episode_id, stretch)";
                StepSql::new(
                    vec![
                        format!(
                            "{name}_numbered AS (SELECT {columns}, \
                             ROW_NUMBER() OVER (PARTITION BY episode_id ORDER BY start_ns) AS position FROM {})",
                            input.name()
                        ),
                        format!(
                            "{name}_passing AS (SELECT {columns}, \
                             position - ROW_NUMBER() OVER (PARTITION BY episode_id ORDER BY start_ns) AS stretch \
                             FROM {name}_numbered WHERE {predicate})"
                        ),
                        format!(
                            "{name}_stretches AS (SELECT episode_id, MIN(start_ns) {stretch} AS start_ns, \
                             MAX(end_ns) {stretch} AS end_ns, AVG({rate}) {stretch} AS {rate}{}, \
                             ROW_NUMBER() OVER (PARTITION BY episode_id, stretch ORDER BY start_ns) AS place \
                             FROM {name}_passing)",
                            input.answers().following("")
                        ),
                        format!(
                            "{name} AS (SELECT {columns} FROM {name}_stretches WHERE place = 1)"
                        ),
                    ],
                    input.renamed(name).publishing(rate),
                )
            }
            Self::Similar { .. } | Self::Categories { .. } | Self::ObjectCount { .. } => {
                StepSql::new(vec![kept], input.renamed(name))
            }
        })
    }
}

#[cfg(test)]
impl Selection {
    /// Episodes whose one recorded fact compares with `value` as asked.
    pub(crate) fn episodes_where(fact: EpisodeFact, comparison: Comparison, value: f64) -> Self {
        Self::Episodes {
            conditions: vec![EpisodeCondition::new(fact, comparison, value)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    /// What a search's rule judges below Break down: the subtask and its
    /// similarity.
    fn searched() -> Relation {
        Relation::new("breakdown", vec![SubtaskRow::DESCRIPTION])
            .measuring("search", SubtaskRow::SIMILARITY)
    }

    /// The same below a grouping: the subtask and its category.
    fn grouped() -> Relation {
        Relation::new("breakdown", vec![SubtaskRow::DESCRIPTION])
            .measuring("grouping", SubtaskRow::CATEGORY)
    }

    /// The predicate as rendered, read back: `column >= CAST(n AS DOUBLE)`,
    /// `column <= …`, or the moments' `(column IS NULL OR column >= …)`, over
    /// one row's value of that column. Anything else is not a predicate this
    /// module renders, and fails the test that reads it.
    struct RenderedPredicate(String);

    /// A rule of categories as rendered, read back:
    /// `relation.column.category IN ('a', 'b')`, over one row's category. A
    /// row without one is NULL in SQL, which `IN` never keeps.
    struct RenderedCategories(String);

    impl RenderedCategories {
        fn keeps(&self, qualified: &str, category: Option<&str>) -> bool {
            let names = self
                .0
                .strip_prefix(&format!("{qualified}.category IN ("))
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or_else(|| panic!("a list of categories: {}", self.0));
            let names = names
                .split(", ")
                .map(|name| {
                    name.strip_prefix('\'')
                        .and_then(|name| name.strip_suffix('\''))
                        .expect("a quoted name")
                        .replace("''", "'")
                })
                .collect::<Vec<_>>();
            category.is_some_and(|category| names.iter().any(|name| name == category))
        }
    }

    impl RenderedPredicate {
        fn keeps(&self, column: &str, value: Option<f64>) -> bool {
            let comparison = |text: &str| {
                let (name, rest) = text.split_once(' ').expect("a column");
                assert_eq!(name, column, "{}", self.0);
                let (operator, literal) = rest.split_once(' ').expect("an operator");
                let threshold = literal
                    .strip_prefix("CAST(")
                    .and_then(|literal| literal.strip_suffix(" AS DOUBLE)"))
                    .expect("a double literal")
                    .parse::<f64>()
                    .expect("a number");
                value.is_some_and(|value| match operator {
                    ">=" => value >= threshold,
                    "<=" => value <= threshold,
                    other => panic!("operator {other}"),
                })
            };
            match self
                .0
                .strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .and_then(|inner| inner.split_once(" OR "))
            {
                Some((unread, rule)) => {
                    assert_eq!(unread, format!("{column} IS NULL"), "{}", self.0);
                    value.is_none() || comparison(rule)
                }
                None => comparison(&self.0),
            }
        }
    }

    #[test]
    fn a_count_rule_keeps_in_the_browser_what_its_predicate_keeps() {
        let counted = Relation::new("episodes", Vec::new()).measuring("track", ObjectCount::COLUMN);
        let range = Selection::ObjectCount {
            fewest: 3,
            most: Some(4),
        };
        assert_eq!(
            range.predicate(&counted).unwrap(),
            "object_count >= 3 AND object_count <= 4"
        );
        // An episode the tracker could not count is NULL in SQL, which no
        // comparison keeps.
        assert_eq!(
            [None, Some(2), Some(3), Some(4), Some(5)].map(|count| range.keeps_count(count)),
            [false, false, true, true, false]
        );
        let any = Selection::for_basis(SelectionBasis::ObjectCount);
        assert_eq!(any.predicate(&counted).unwrap(), "object_count >= 0");
        assert!(any.keeps_count(Some(0)) && !any.keeps_count(None));
        // A range given backwards is still a range.
        assert_eq!(
            range.with_count_range((6, 2)),
            Selection::ObjectCount {
                fewest: 2,
                most: Some(6)
            }
        );
    }

    #[test]
    fn the_browser_filter_and_the_rendered_predicate_keep_the_same_rows() {
        // Episodes either side of each threshold, on it, and without the fact.
        let episodes = [
            9_999_999_999_u64,
            10_000_000_000,
            10_000_000_001,
            20_500_000_000,
        ]
        .into_iter()
        .zip(1..)
        .map(|(duration, ordinal)| Episode::fixture(&EpisodeId::fixture(ordinal), duration))
        .collect::<Vec<_>>();
        for condition in [
            EpisodeCondition::new(EpisodeFact::DurationSeconds, Comparison::AtLeast, 10.0),
            EpisodeCondition::new(
                EpisodeFact::DurationSeconds,
                Comparison::AtMost,
                10.000_000_001,
            ),
            EpisodeCondition::new(EpisodeFact::Messages, Comparison::AtLeast, 0.0),
        ] {
            let rule = Selection::Episodes {
                conditions: vec![condition],
            };
            let fact = condition.fact();
            let rendered = RenderedPredicate(rule.predicate(&searched()).unwrap());
            for episode in &episodes {
                assert_eq!(
                    rule.keeps_episode(episode),
                    rendered.keeps(fact.key(), fact.of(episode)),
                    "{} on {:?}",
                    rendered.0,
                    fact.of(episode)
                );
            }
        }
        // Moments below, on, and above the threshold, and one never read.
        for minimum in [-0.5, 0.0, 0.2] {
            let rule = Selection::Progressing { minimum };
            let rendered = RenderedPredicate(rule.predicate(&searched()).unwrap());
            for rate in [
                Some(-0.6),
                Some(-0.5),
                Some(0.0),
                Some(0.19),
                Some(0.2),
                Some(1.4),
                None,
            ] {
                assert_eq!(
                    rule.keeps_moment(rate),
                    rendered.keeps(Moment::PROGRESS_RATE, rate),
                    "{} on {rate:?}",
                    rendered.0
                );
            }
        }
        // Subtasks below, on, and above a similarity, and one the model could
        // not judge, which neither side keeps.
        for minimum in [0.0, 0.5, 0.7] {
            let rule = Selection::Similar { minimum };
            let rendered = RenderedPredicate(rule.predicate(&searched()).unwrap());
            for similarity in [None, Some(0.0), Some(0.49), Some(0.5), Some(0.7), Some(1.0)] {
                assert_eq!(
                    rule.keeps_subtask(similarity, None),
                    rendered.keeps(SubtaskRow::SIMILARITY, similarity),
                    "{} on {similarity:?}",
                    rendered.0
                );
            }
        }
        // Subtasks in a kept category, in another, and in none; a name with a
        // quote in it survives the quoting.
        let rule = Selection::Categories {
            names: vec!["fold".into(), "fold's edge".into()],
        };
        let rendered = RenderedCategories(rule.predicate(&grouped()).unwrap());
        for category in [
            None,
            Some("fold"),
            Some("fold's edge"),
            Some("grasp"),
            Some("Fold"),
        ] {
            assert_eq!(
                rule.keeps_subtask(None, category),
                rendered.keeps("grouping.subtask_category", category),
                "{} on {category:?}",
                rendered.0
            );
        }
        // Choosing no category keeps nothing, so there is nothing to publish.
        assert!(
            Selection::Categories { names: Vec::new() }
                .predicate(&grouped())
                .is_err()
        );
        // A rule over a measure nothing above took has no column to read.
        assert!(
            Selection::Similar { minimum: 0.5 }
                .predicate(&grouped())
                .is_err()
        );
    }

    #[test]
    fn an_episode_is_kept_when_it_meets_every_condition() {
        let rule = Selection::Episodes {
            conditions: vec![
                EpisodeCondition::new(EpisodeFact::DurationSeconds, Comparison::AtLeast, 10.0),
                EpisodeCondition::new(EpisodeFact::DurationSeconds, Comparison::AtMost, 20.0),
            ],
        };
        let lasting = |seconds: u64| {
            rule.keeps_episode(&Episode::fixture(
                &EpisodeId::fixture(1),
                seconds * 1_000_000_000,
            ))
        };
        assert!(!lasting(9) && lasting(10) && lasting(20) && !lasting(21));
        assert_eq!(
            rule.predicate(&searched()).unwrap(),
            "duration_seconds >= CAST(10 AS DOUBLE) AND duration_seconds <= CAST(20 AS DOUBLE)"
        );
        assert_eq!(
            rule.sentence(),
            "Duration at least 10 s and duration at most 20 s"
        );
    }

    #[test]
    fn a_rule_reads_the_nearest_measure_of_its_kind() {
        // A second search below the first writes its own column, and the
        // Select below it reads that one.
        let twice = searched().measuring("again", SubtaskRow::SIMILARITY);
        assert_eq!(
            Selection::Similar { minimum: 0.5 }
                .predicate(&twice)
                .unwrap(),
            "similarity_2 >= CAST(0.5 AS DOUBLE)"
        );
    }

    #[test]
    fn a_new_select_keeps_everything_measured_and_says_so() {
        let episodes = Selection::for_basis(SelectionBasis::EpisodeFacts);
        let episode = Episode::fixture(&EpisodeId::fixture(1), 12_000_000_000);
        assert!(episodes.keeps_episode(&episode));
        assert_eq!(episodes.sentence(), "Duration at least 0 s");
        assert!(Selection::for_basis(SelectionBasis::Progress).keeps_moment(Some(0.0)));
        assert_eq!(
            Selection::for_basis(SelectionBasis::Categories),
            Selection::Categories { names: Vec::new() }
        );
        for basis in [
            SelectionBasis::EpisodeFacts,
            SelectionBasis::Similarity,
            SelectionBasis::Categories,
            SelectionBasis::Progress,
        ] {
            assert_eq!(Selection::for_basis(basis).basis(), basis);
        }
    }

    #[test]
    fn each_rule_keeps_by_its_own_measure_and_nothing_unmeasured() {
        let longer = Selection::Episodes {
            conditions: vec![EpisodeCondition::new(
                EpisodeFact::DurationSeconds,
                Comparison::AtLeast,
                20.0,
            )],
        };
        assert!(!longer.keeps_episode(&Episode::fixture(&EpisodeId::fixture(1), 19_000_000_000)));
        assert!(longer.keeps_episode(&Episode::fixture(&EpisodeId::fixture(1), 20_000_000_000)));
        assert_eq!(longer.sentence(), "Duration at least 20 s");

        let similar = Selection::Similar { minimum: 0.7 };
        assert!(similar.keeps_subtask(Some(0.7), None));
        assert!(!similar.keeps_subtask(Some(0.69), None));
        assert!(!similar.keeps_subtask(None, Some("Fold")));

        let folds = Selection::Categories {
            names: vec!["Fold".into(), "Smooth".into()],
        };
        assert!(folds.keeps_subtask(None, Some("Smooth")));
        assert!(!folds.keeps_subtask(Some(1.0), Some("Grasp")));
        assert!(!folds.keeps_subtask(Some(1.0), None));
        assert_eq!(folds.sentence(), "Fold, Smooth");

        let moving = Selection::Progressing { minimum: 0.2 };
        assert!(!moving.keeps_moment(Some(0.1)));
        assert!(moving.keeps_moment(Some(0.2)));
        assert!(moving.keeps_moment(None), "an unread moment is kept");
    }

    #[test]
    fn a_rule_is_stored_in_its_own_words_and_checked() {
        let json = serde_json::to_value(Selection::Episodes {
            conditions: vec![EpisodeCondition::new(
                EpisodeFact::Messages,
                Comparison::AtMost,
                5000.0,
            )],
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"keep": "episodes", "conditions": [{"fact": "messages", "comparison": "at_most", "value": 5000.0}]})
        );
        assert!(
            Selection::Episodes {
                conditions: Vec::new()
            }
            .validate()
            .is_err()
        );
        assert!(Selection::Similar { minimum: 1.2 }.validate().is_err());
        assert!(
            Selection::Categories {
                names: vec!["Fold".into(), "Fold".into()]
            }
            .validate()
            .is_err()
        );
        assert!(
            Selection::Episodes {
                conditions: vec![EpisodeCondition::new(
                    EpisodeFact::Channels,
                    Comparison::AtLeast,
                    f64::NAN,
                )],
            }
            .validate()
            .is_err()
        );
    }
}
