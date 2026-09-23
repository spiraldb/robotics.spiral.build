//! What a Select node keeps of the population it receives.
//!
//! A Select narrows; it never measures. What it can keep by is decided by the
//! nearest node above it that is not itself a Select — its basis: the
//! dataset's recorded episode facts, a search's probabilities, a grouping's
//! categories, or a detection's progress. The rule is stored in those terms,
//! so a Select reads as one sentence about the node it follows.
use crate::activities::episode::Episode;
use serde::{Deserialize, Serialize};

/// What a Select's input was measured by, and so what its rule can name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionBasis {
    /// The dataset's own statistics of each episode.
    EpisodeFacts,
    /// A Find similar node's probability for each subtask.
    Similarity,
    /// A Group similar node's category for each subtask.
    Categories,
    /// An Identify unproductive moments node's progress for each moment.
    Progress,
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
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration",
            Self::Messages => "messages",
            Self::Channels => "channels",
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
    /// The fact's value for one episode, when Spiral listed it.
    pub(crate) fn of(self, episode: &Episode) -> Option<f64> {
        let facts = episode.facts();
        match self {
            Self::DurationSeconds => facts.duration_seconds(),
            Self::Messages => facts.message_count().map(|count| count as f64),
            Self::Channels => facts.channel_count().map(|count| count as f64),
            Self::SizeMegabytes => facts.source_bytes().map(|bytes| bytes as f64 / 1e6),
        }
    }
}

/// Which side of the threshold a Select keeps.
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
        Self::ALL.into_iter().find(|comparison| comparison.key() == key)
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
}

/// One Select's rule, in the terms of its basis.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "keep", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Selection {
    /// Episodes whose recorded fact compares with `value` as asked.
    Episodes {
        fact: EpisodeFact,
        comparison: Comparison,
        value: f64,
    },
    /// Subtasks at least this likely to be the operation a search asked about.
    Similar { minimum: f64 },
    /// Subtasks in any of these categories.
    Categories { names: Vec<String> },
    /// Moments progressing at least this fast. A moment the scorer could not
    /// read is kept: an unread moment is not an unproductive one.
    Progressing { minimum: f64 },
}

impl Selection {
    /// A new Select's rule. Thresholds start where they keep everything
    /// measured, so adding a Select never silently drops data; categories
    /// start empty, because a grouping has no category that means "all".
    pub(crate) fn for_basis(basis: SelectionBasis) -> Self {
        match basis {
            SelectionBasis::EpisodeFacts => Self::Episodes {
                fact: EpisodeFact::DurationSeconds,
                comparison: Comparison::AtLeast,
                value: 0.0,
            },
            SelectionBasis::Similarity => Self::Similar { minimum: 0.5 },
            SelectionBasis::Categories => Self::Categories { names: Vec::new() },
            SelectionBasis::Progress => Self::Progressing { minimum: 0.0 },
        }
    }
    pub(crate) fn basis(&self) -> SelectionBasis {
        match self {
            Self::Episodes { .. } => SelectionBasis::EpisodeFacts,
            Self::Similar { .. } => SelectionBasis::Similarity,
            Self::Categories { .. } => SelectionBasis::Categories,
            Self::Progressing { .. } => SelectionBasis::Progress,
        }
    }
    /// Whether an episode is kept, by its recorded facts.
    pub(crate) fn keeps_episode(&self, episode: &Episode) -> bool {
        match self {
            Self::Episodes {
                fact,
                comparison,
                value,
            } => fact
                .of(episode)
                .is_some_and(|measured| comparison.holds(measured, *value)),
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
    /// Whether a moment is kept, by its mean progress.
    pub(crate) fn keeps_moment(&self, velocity: Option<f64>) -> bool {
        match self {
            Self::Progressing { minimum } => velocity.is_none_or(|value| value >= *minimum),
            _ => false,
        }
    }
    /// The rule as its node card says it.
    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::Episodes {
                fact,
                comparison,
                value,
            } => {
                let unit = fact.unit().map(|unit| format!(" {unit}")).unwrap_or_default();
                let fact = fact.label();
                let mut sentence = format!("{fact} {} {value}{unit}", comparison.label());
                sentence[..1].make_ascii_uppercase();
                sentence
            }
            Self::Similar { minimum } => format!("At least {:.0}% similar", minimum * 100.0),
            Self::Categories { names } if names.is_empty() => "No category chosen".to_owned(),
            Self::Categories { names } => names.join(", "),
            Self::Progressing { minimum } => format!("Progress at least {minimum}"),
        }
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Episodes { value, .. } if !value.is_finite() => {
                Err("A Select's threshold must be a number.".into())
            }
            Self::Similar { minimum } if !(0.0..=1.0).contains(minimum) => {
                Err("A similarity threshold must be between 0% and 100%.".into())
            }
            Self::Progressing { minimum } if !minimum.is_finite() => {
                Err("A progress threshold must be a number.".into())
            }
            Self::Categories { names } => {
                let mut seen = std::collections::BTreeSet::new();
                for name in names {
                    if name.trim().is_empty() || !seen.insert(name.as_str()) {
                        return Err("A Select names each category once.".into());
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    #[test]
    fn a_new_select_keeps_everything_measured_and_says_so() {
        let episodes = Selection::for_basis(SelectionBasis::EpisodeFacts);
        let episode = Episode::fixture(&EpisodeId::fixture(1), 12_000_000_000);
        assert!(episodes.keeps_episode(&episode));
        assert_eq!(episodes.sentence(), "Duration at least 0 s");
        assert!(Selection::for_basis(SelectionBasis::Progress).keeps_moment(Some(0.0)));
        assert_eq!(
            Selection::for_basis(SelectionBasis::Categories).sentence(),
            "No category chosen"
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
            fact: EpisodeFact::DurationSeconds,
            comparison: Comparison::AtLeast,
            value: 20.0,
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
            fact: EpisodeFact::Messages,
            comparison: Comparison::AtMost,
            value: 5000.0,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"keep": "episodes", "fact": "messages", "comparison": "at_most", "value": 5000.0})
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
                fact: EpisodeFact::Channels,
                comparison: Comparison::AtLeast,
                value: f64::NAN,
            }
            .validate()
            .is_err()
        );
    }
}
