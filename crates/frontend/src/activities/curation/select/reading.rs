//! A Select's conditions read in the browser, over the sample: the same
//! episodes and ranges its SQL keeps (`step.rs`), from the sample's facts
//! and what the previews above it read, so sliding a threshold answers at
//! once and asks nothing again.
use crate::activities::curation::document::{
    Comparison, CurationNode, CurationNodeKind, EpisodeCondition, EpisodeNumber, SelectCondition,
    Selection,
};
use crate::activities::curation::population::{KeptEpisode, Population, PopulationReader};
use spiral_robotics_common::datasets::EpisodeId;

/// What one condition makes of the episodes reaching its Select, in their
/// order.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum ConditionReading {
    /// Each episode's number, and whether the condition keeps the episode.
    Episodes {
        numbers: Vec<Option<f64>>,
        holds: Vec<bool>,
    },
    /// The ranges the condition keeps of each episode.
    Ranges(Vec<Vec<(f64, f64)>>),
}

impl ConditionReading {
    /// Whether the condition keeps the reaching episode at `place`, or any
    /// of it.
    pub(in crate::activities::curation) fn holds(&self, place: usize) -> bool {
        match self {
            Self::Episodes { holds, .. } => holds.get(place).copied().unwrap_or(false),
            Self::Ranges(ranges) => ranges.get(place).is_some_and(|ranges| !ranges.is_empty()),
        }
    }
}

/// What a Select makes of what reaches it: every condition's reading, and
/// what it keeps.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct SelectReading {
    input: Population,
    conditions: Vec<ConditionReading>,
    output: Population,
}

impl SelectReading {
    pub(in crate::activities::curation) fn input(&self) -> &Population {
        &self.input
    }
    pub(in crate::activities::curation) fn output(&self) -> &Population {
        &self.output
    }
    pub(in crate::activities::curation) fn condition(
        &self,
        index: usize,
    ) -> Option<&ConditionReading> {
        self.conditions.get(index)
    }
    /// How much of what reaches the Select the range condition at `index`
    /// keeps, whatever the others say.
    pub(in crate::activities::curation) fn seconds_kept_by(&self, index: usize) -> Option<f64> {
        let Some(ConditionReading::Ranges(within)) = self.condition(index) else {
            return None;
        };
        Some(
            self.input
                .kept_episodes()
                .iter()
                .zip(within)
                .map(|(kept, within)| kept.kept().within(within).seconds())
                .sum(),
        )
    }
    /// How many reaching episodes the condition at `index` leaves out
    /// whole, whatever the others say.
    pub(in crate::activities::curation) fn leaves_out(&self, index: usize) -> usize {
        self.condition(index).map_or(0, |reading| {
            (0..self.input.len())
                .filter(|place| !reading.holds(*place))
                .count()
        })
    }
}

impl Selection {
    /// What this Select keeps of `input`.
    pub(in crate::activities::curation) fn keep(
        &self,
        input: &Population,
        reader: &PopulationReader<'_>,
    ) -> Population {
        self.read(input.clone(), reader).output
    }

    /// Every condition over `input`, each judged over all of it, and what
    /// is kept: the episodes every episode condition keeps, their kept
    /// ranges narrowed by each range condition in turn, and none with
    /// nothing left.
    pub(in crate::activities::curation) fn read(
        &self,
        input: Population,
        reader: &PopulationReader<'_>,
    ) -> SelectReading {
        let episodes = input.episodes();
        let conditions = self
            .conditions()
            .iter()
            .map(|condition| condition.read(&episodes, reader))
            .collect::<Vec<_>>();
        let output = input
            .kept_episodes()
            .iter()
            .enumerate()
            .filter_map(|(place, kept)| {
                let mut ranges = kept.kept().clone();
                for reading in &conditions {
                    match reading {
                        ConditionReading::Episodes { holds, .. } if !holds[place] => return None,
                        ConditionReading::Episodes { .. } => {}
                        ConditionReading::Ranges(within) => ranges = ranges.within(&within[place]),
                    }
                }
                let narrowed = conditions
                    .iter()
                    .any(|reading| matches!(reading, ConditionReading::Ranges(_)));
                (!narrowed || !ranges.is_empty())
                    .then(|| KeptEpisode::new(kept.episode().clone(), ranges))
            })
            .collect();
        SelectReading {
            input,
            conditions,
            output: Population::new(output),
        }
    }
}

/// One thing a range condition judges of an episode — a moment of a
/// timeline, or a subtask — where it lies, its value, and whether the
/// condition passes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::activities::curation) struct JudgedItem {
    span: (f64, f64),
    value: Option<f64>,
    passes: bool,
}

impl JudgedItem {
    pub(in crate::activities::curation) fn span(&self) -> (f64, f64) {
        self.span
    }
    pub(in crate::activities::curation) fn value(&self) -> Option<f64> {
        self.value
    }
    pub(in crate::activities::curation) fn passes(&self) -> bool {
        self.passes
    }
}

impl SelectCondition {
    fn read(&self, episodes: &[EpisodeId], reader: &PopulationReader<'_>) -> ConditionReading {
        let Self::Episodes(condition) = self else {
            return ConditionReading::Ranges(
                episodes
                    .iter()
                    .map(|episode| self.ranges(episode, reader))
                    .collect(),
            );
        };
        let numbers = episodes
            .iter()
            .map(|episode| condition.number().of(episode, reader))
            .collect::<Vec<_>>();
        let holds = condition.holds(&numbers);
        ConditionReading::Episodes { numbers, holds }
    }

    /// The ranges this range condition keeps of `episode`: a timeline's
    /// passing moments joined into stretches, or each passing subtask.
    fn ranges(&self, episode: &EpisodeId, reader: &PopulationReader<'_>) -> Vec<(f64, f64)> {
        match self {
            Self::Progressing { node, .. } | Self::Visible { node, .. } => reader
                .timeline(node, episode)
                .map_or_else(Vec::new, |timeline| {
                    timeline.stretches(|value| self.passes(value, None))
                }),
            _ => self
                .judged_items(episode, reader)
                .iter()
                .filter(|item| item.passes)
                .map(|item| item.span)
                .collect(),
        }
    }

    /// Everything this range condition judges of `episode`, in time order:
    /// a timeline's moments, or the subtasks of the node it reads; nothing
    /// for an episode condition.
    pub(in crate::activities::curation) fn judged_items(
        &self,
        episode: &EpisodeId,
        reader: &PopulationReader<'_>,
    ) -> Vec<JudgedItem> {
        match self {
            Self::Episodes(_) => Vec::new(),
            Self::Progressing { node, .. } | Self::Visible { node, .. } => reader
                .timeline(node, episode)
                .map(|timeline| {
                    timeline
                        .moments()
                        .iter()
                        .map(|moment| JudgedItem {
                            span: (moment.start(), moment.end()),
                            value: moment.value(),
                            passes: self.passes(moment.value(), None),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Self::Similar { node, .. } | Self::InCategories { node, .. } => {
                let names = self.category_names(reader);
                reader
                    .subtasks(node, episode)
                    .iter()
                    .map(|row| JudgedItem {
                        span: (row.start_time(), row.end_time()),
                        value: row.similarity(),
                        passes: self.passes(
                            row.similarity(),
                            row.category().map(|category| (category, names.as_slice())),
                        ),
                    })
                    .collect()
            }
        }
    }

    /// Whether this range condition passes a moment or subtask holding
    /// `value`, or placed in a category among the chosen `names`: a moment
    /// the scorer could not read progresses, and nothing unjudged is
    /// similar or counted.
    fn passes(&self, value: Option<f64>, category: Option<(&str, &[String])>) -> bool {
        match self {
            Self::Episodes(_) => false,
            Self::Progressing { minimum, .. } => value.is_none_or(|rate| rate >= *minimum),
            Self::Visible { fewest, .. } => value.is_some_and(|count| count >= *fewest as f64),
            Self::Similar { minimum, .. } => value.is_some_and(|similarity| similarity >= *minimum),
            Self::InCategories { .. } => {
                category.is_some_and(|(category, names)| names.iter().any(|name| name == category))
            }
        }
    }

    /// The names of the categories an `InCategories` condition chooses.
    fn category_names(&self, reader: &PopulationReader<'_>) -> Vec<String> {
        let Self::InCategories { node, categories } = self else {
            return Vec::new();
        };
        match reader.graph().node(node).map(CurationNode::kind) {
            Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => categories
                .iter()
                .filter_map(|id| grouping.category(*id))
                .map(|category| category.name().to_owned())
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl EpisodeNumber {
    /// This number of `episode`, as the SQL derives it: a fact as Spiral
    /// listed it, the most objects one tracked picture held, or the
    /// duration per that most when it is more than none.
    pub(in crate::activities::curation) fn of(
        &self,
        episode: &EpisodeId,
        reader: &PopulationReader<'_>,
    ) -> Option<f64> {
        let most = |node: &str| reader.timeline(node, episode)?.most();
        match self {
            Self::Fact(fact) => fact.of(reader.sample().episode(episode)?),
            Self::MostAtOnce(node) => most(node),
            Self::DurationPer(node) => {
                let most = most(node).filter(|most| *most > 0.0)?;
                Some(reader.sample().duration(episode)? / most)
            }
        }
    }
}

impl EpisodeCondition {
    /// Whether it keeps each of `numbers`, the reaching episodes' numbers in
    /// their order. A ranked condition ranks each among those that have one:
    /// how many come before it, over their count less one, as SQL's
    /// `PERCENT_RANK`. An episode without the number is never kept.
    pub(in crate::activities::curation) fn holds(&self, numbers: &[Option<f64>]) -> Vec<bool> {
        let ranks = self.ranks(numbers);
        numbers
            .iter()
            .zip(ranks)
            .map(|(number, rank)| match (self.comparison(), number, rank) {
                (Comparison::AtLeast, Some(number), _) => *number >= self.value(),
                (Comparison::AtMost, Some(number), _) => *number <= self.value(),
                (Comparison::Lowest | Comparison::Highest, _, Some(rank)) => rank <= self.value(),
                _ => false,
            })
            .collect()
    }

    /// Each of `numbers`' percent rank from the end a ranked condition keeps,
    /// among those that have one; none for a condition that does not rank.
    pub(in crate::activities::curation) fn ranks(
        &self,
        numbers: &[Option<f64>],
    ) -> Vec<Option<f64>> {
        let present = numbers.iter().flatten().copied().collect::<Vec<_>>();
        let spread = (present.len().max(2) - 1) as f64;
        numbers
            .iter()
            .map(|number| {
                let number = (*number)?;
                let before = present
                    .iter()
                    .filter(|other| match self.comparison() {
                        Comparison::Lowest => **other < number,
                        Comparison::Highest => **other > number,
                        Comparison::AtLeast | Comparison::AtMost => false,
                    })
                    .count();
                self.comparison().ranks().then(|| before as f64 / spread)
            })
            .collect()
    }
}
