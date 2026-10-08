//! A Select's conditions, the one filter of a curation. An episode is kept
//! when every condition holds, each judged over the episodes that reach the
//! Select.
//!
//! An episode condition keeps or drops the row: a number per episode — a
//! recorded fact, or one derived from a Track objects column above — compared
//! with a value or ranked among the reaching episodes. A range condition
//! narrows the episode's kept ranges to where a column above passes:
//! progressing fast enough, enough objects in view, a similar subtask, or a
//! chosen category. A condition names the node whose column it reads by its
//! id, so it reads as one sentence about the graph.
//!
//! The conditions have two readings that must agree: the browser keeps what
//! the preview read as a threshold slides (`select/reading.rs`), and the path
//! compiles them to the SQL a published view keeps by (`select/step.rs`).
use super::{CategoryId, CurationGraph, CurationNode, CurationNodeKind};
use crate::activities::episode::Episode;
use crate::format::{ClockPresentation as _, GroupedCountPresentation as _};
use serde::{Deserialize, Serialize};

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
    /// Its column in the curation table.
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration_seconds",
            Self::Messages => "messages",
            Self::Channels => "channels",
            Self::SizeMegabytes => "size_megabytes",
        }
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

/// How an episode condition keeps: by its number against a value, or by
/// where its number ranks among the reaching episodes that have one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Comparison {
    AtLeast,
    AtMost,
    /// Its percent rank from the smallest is at most the value.
    Lowest,
    /// Its percent rank from the largest is at most the value.
    Highest,
}

impl Comparison {
    pub(crate) const ALL: [Self; 4] = [Self::AtLeast, Self::AtMost, Self::Lowest, Self::Highest];
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::AtLeast => "at_least",
            Self::AtMost => "at_most",
            Self::Lowest => "lowest",
            Self::Highest => "highest",
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
            Self::Lowest => "lowest",
            Self::Highest => "highest",
        }
    }
    /// Whether its value is a share of the reaching episodes, 0 to 1.
    pub(crate) const fn ranks(self) -> bool {
        matches!(self, Self::Lowest | Self::Highest)
    }
}

/// A number every episode may have.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EpisodeNumber {
    /// What Spiral recorded of it.
    Fact(EpisodeFact),
    /// The most objects one sampled picture of a Track objects node held.
    MostAtOnce(String),
    /// Its duration per that most.
    DurationPer(String),
}

impl EpisodeNumber {
    /// The Track objects node it reads, when it reads one.
    pub(crate) fn node(&self) -> Option<&str> {
        match self {
            Self::Fact(_) => None,
            Self::MostAtOnce(node) | Self::DurationPer(node) => Some(node),
        }
    }
    /// The number as a condition's row starts with it: "Duration", "Most
    /// bottles at once", "Duration per bottle".
    pub(crate) fn name(&self, graph: &CurationGraph) -> String {
        let tracked = |node: &str| match graph.node(node).map(CurationNode::kind) {
            Some(CurationNodeKind::TrackObjects(tracking)) => tracking.clone(),
            _ => Default::default(),
        };
        match self {
            Self::Fact(fact) => fact.name().to_owned(),
            Self::MostAtOnce(node) => format!("Most {} at once", tracked(node).plural()),
            Self::DurationPer(node) => match tracked(node).prompt() {
                "" => "Duration per object".to_owned(),
                prompt => format!("Duration per {prompt}"),
            },
        }
    }
    /// What a value of it is counted in, when it has a unit.
    pub(crate) fn unit(&self) -> Option<&'static str> {
        match self {
            Self::Fact(fact) => fact.unit(),
            Self::MostAtOnce(_) => None,
            Self::DurationPer(_) => Some("s"),
        }
    }
    /// Whether it counts things, one at a time.
    pub(crate) fn counts(&self) -> bool {
        matches!(self, Self::MostAtOnce(_))
    }
}

/// One condition on a number per episode.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpisodeCondition {
    number: EpisodeNumber,
    comparison: Comparison,
    value: f64,
}

impl EpisodeCondition {
    /// `number` compared as asked; a ranked value is a share, 0 to 1.
    pub(crate) fn new(number: EpisodeNumber, comparison: Comparison, value: f64) -> Self {
        Self {
            number,
            comparison,
            value,
        }
    }
    pub(crate) fn number(&self) -> &EpisodeNumber {
        &self.number
    }
    pub(crate) const fn comparison(&self) -> Comparison {
        self.comparison
    }
    pub(crate) const fn value(&self) -> f64 {
        self.value
    }
    /// The same condition on another number, from where it keeps everything.
    pub(crate) fn on(&self, number: EpisodeNumber) -> Self {
        Self::new(number, self.comparison, self.kept_whole(self.comparison))
    }
    /// The same condition compared another way: a ranked one from where it
    /// keeps everything, since a share and a number do not convert.
    pub(crate) fn comparing(&self, comparison: Comparison) -> Self {
        let value = if comparison.ranks() == self.comparison.ranks() {
            self.value
        } else {
            self.kept_whole(comparison)
        };
        Self::new(self.number.clone(), comparison, value)
    }
    pub(crate) fn at(&self, value: f64) -> Self {
        Self::new(self.number.clone(), self.comparison, value)
    }
    /// Where `comparison` keeps every episode that has the number: all of
    /// them ranked, from nothing at least.
    fn kept_whole(&self, comparison: Comparison) -> f64 {
        match comparison {
            Comparison::Lowest | Comparison::Highest => 1.0,
            Comparison::AtLeast | Comparison::AtMost => 0.0,
        }
    }
    /// "duration per bottle in the lowest 25%", "duration at least 20 s".
    fn sentence(&self, graph: &CurationGraph) -> String {
        let number = self.number.name(graph).to_lowercase();
        if self.comparison.ranks() {
            return format!(
                "{number} in the {} {:.0}%",
                self.comparison.label(),
                self.value * 100.0
            );
        }
        let unit = self
            .number
            .unit()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        format!("{number} {} {}{unit}", self.comparison.label(), self.value)
    }
    fn validate(&self) -> Result<(), String> {
        if !self.value.is_finite() {
            return Err("A Select's threshold must be a number.".into());
        }
        if self.comparison.ranks() && !(0.0..=1.0).contains(&self.value) {
            return Err("A share of the episodes is between 0% and 100%.".into());
        }
        Ok(())
    }
}

/// One condition of a Select.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "keep", rename_all = "snake_case")]
pub(crate) enum SelectCondition {
    /// Episodes whose number passes.
    Episodes(EpisodeCondition),
    /// The moments of a Score progress timeline progressing at least
    /// `minimum`, and those the scorer could not read: an unread moment is
    /// not a slow one.
    Progressing { node: String, minimum: f64 },
    /// The moments of a Track objects timeline whose picture holds at least
    /// `fewest` objects.
    Visible { node: String, fewest: u64 },
    /// The subtasks of a Find similar column at least `minimum` similar. A
    /// subtask the model could not judge is not kept.
    Similar { node: String, minimum: f64 },
    /// The subtasks a Group similar node put in any of `categories`.
    InCategories {
        node: String,
        categories: Vec<CategoryId>,
    },
}

impl SelectCondition {
    /// The node whose column the condition reads, when it reads one.
    pub(crate) fn node(&self) -> Option<&str> {
        match self {
            Self::Episodes(condition) => condition.number.node(),
            Self::Progressing { node, .. }
            | Self::Visible { node, .. }
            | Self::Similar { node, .. }
            | Self::InCategories { node, .. } => Some(node),
        }
    }
    /// Whether it narrows the kept ranges rather than keeping or dropping
    /// whole rows.
    pub(crate) fn narrows(&self) -> bool {
        !matches!(self, Self::Episodes(_))
    }
    /// What the condition keeps by, as its row and the menu adding it name
    /// it: "Duration per bottle", "Progress rate", "Bottles visible".
    pub(crate) fn name(&self, graph: &CurationGraph) -> String {
        match self {
            Self::Episodes(condition) => condition.number.name(graph),
            Self::Progressing { .. } => "Progress rate".to_owned(),
            Self::Visible { node, .. } => {
                let plural = Self::tracking(graph, node).plural();
                let mut letters = plural.chars();
                letters.next().map_or_else(String::new, |first| {
                    format!("{}{} visible", first.to_uppercase(), letters.as_str())
                })
            }
            Self::Similar { node, .. } => match graph.node(node).map(CurationNode::kind) {
                Some(CurationNodeKind::FindSimilarSubtasks(search)) => {
                    format!("Similar to “{}”", search.query().trim())
                }
                _ => "Similarity".to_owned(),
            },
            Self::InCategories { .. } => "Category".to_owned(),
        }
    }
    /// The condition as the node card says it: "progress rate at least
    /// 0.20×".
    pub(crate) fn sentence(&self, graph: &CurationGraph) -> String {
        match self {
            Self::Episodes(condition) => condition.sentence(graph),
            Self::Progressing { minimum, .. } => format!("progress rate at least {minimum:.2}×"),
            Self::Visible { node, fewest } => format!(
                "at least {fewest} {} visible",
                Self::tracking(graph, node).plural()
            ),
            Self::Similar { minimum, .. } => {
                format!("at least {:.0}% similar", minimum * 100.0)
            }
            Self::InCategories { node, categories } => {
                let grouping = Self::grouping(graph, node);
                let names = categories
                    .iter()
                    .filter_map(|id| grouping.category(*id))
                    .map(|category| category.name().to_owned())
                    .collect::<Vec<_>>();
                match names.as_slice() {
                    [] => "in no category".to_owned(),
                    names => format!("in {}", names.join(", ")),
                }
            }
        }
    }
    fn tracking(graph: &CurationGraph, node: &str) -> super::ObjectTracking {
        match graph.node(node).map(CurationNode::kind) {
            Some(CurationNodeKind::TrackObjects(tracking)) => tracking.clone(),
            _ => Default::default(),
        }
    }
    fn grouping(graph: &CurationGraph, node: &str) -> super::SubtaskGrouping {
        match graph.node(node).map(CurationNode::kind) {
            Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => grouping.clone(),
            _ => Default::default(),
        }
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Episodes(condition) => condition.validate(),
            Self::Progressing { minimum, .. } if !minimum.is_finite() => {
                Err("A progress threshold must be a number.".into())
            }
            Self::Similar { minimum, .. } if !(0.0..=1.0).contains(minimum) => {
                Err("A similarity threshold must be between 0% and 100%.".into())
            }
            Self::InCategories { categories, .. } => {
                let mut seen = std::collections::BTreeSet::new();
                if categories.iter().any(|id| !seen.insert(*id)) {
                    return Err("A Select chooses each category once.".into());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    /// Whether the node it reads is `above`, of the kind it reads, and has
    /// every category it chooses.
    fn reads(&self, above: &CurationNode) -> bool {
        match (self, above.kind()) {
            (Self::Episodes(_) | Self::Visible { .. }, CurationNodeKind::TrackObjects(_))
            | (Self::Progressing { .. }, CurationNodeKind::ScoreProgress(_))
            | (Self::Similar { .. }, CurationNodeKind::FindSimilarSubtasks(_)) => true,
            (
                Self::InCategories { categories, .. },
                CurationNodeKind::GroupSimilarSubtasks(grouping),
            ) => categories.iter().all(|id| grouping.category(*id).is_some()),
            _ => false,
        }
    }
}

/// A condition the Select may add, and what its menu calls it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConditionChoice {
    label: String,
    condition: SelectCondition,
}

impl ConditionChoice {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
    pub(crate) fn condition(&self) -> &SelectCondition {
        &self.condition
    }
}

/// A Select: every condition an episode it keeps meets.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    conditions: Vec<SelectCondition>,
}

impl Selection {
    /// A new Select keeps every episode, so adding one never silently drops
    /// data: its one condition is a duration of at least nothing.
    pub(crate) fn keeping_everything() -> Self {
        Self {
            conditions: vec![SelectCondition::Episodes(EpisodeCondition::new(
                EpisodeNumber::Fact(EpisodeFact::DurationSeconds),
                Comparison::AtLeast,
                0.0,
            ))],
        }
    }
    pub(crate) fn conditions(&self) -> &[SelectCondition] {
        &self.conditions
    }
    /// The same Select with its condition at `index` replaced.
    pub(crate) fn replacing(&self, index: usize, condition: SelectCondition) -> Self {
        let mut conditions = self.conditions.clone();
        if let Some(slot) = conditions.get_mut(index) {
            *slot = condition;
        }
        Self { conditions }
    }
    /// The same Select with one more condition.
    pub(crate) fn adding(&self, condition: SelectCondition) -> Self {
        let mut conditions = self.conditions.clone();
        conditions.push(condition);
        Self { conditions }
    }
    /// The same Select without its condition at `index`.
    pub(crate) fn without(&self, index: usize) -> Self {
        let mut conditions = self.conditions.clone();
        if index < conditions.len() {
            conditions.remove(index);
        }
        Self { conditions }
    }
    /// The conditions as the node card says them: "Duration per bottle in
    /// the lowest 25% and progress rate at least 0.20×".
    pub(crate) fn sentence(&self, graph: &CurationGraph) -> String {
        let mut sentence = self
            .conditions
            .iter()
            .map(|condition| condition.sentence(graph))
            .collect::<Vec<_>>()
            .join(" and ");
        if let Some(first) = sentence.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        sentence
    }
    /// A grouping's categories changed: each condition choosing among
    /// `node`'s chooses what `rechoose` makes of its choice, when it makes
    /// anything.
    pub(super) fn rechoose_categories(
        &mut self,
        node: &str,
        rechoose: &impl Fn(&[CategoryId]) -> Option<Vec<CategoryId>>,
    ) {
        for condition in &mut self.conditions {
            if let SelectCondition::InCategories {
                node: chosen_from,
                categories,
            } = condition
                && chosen_from == node
                && let Some(chosen) = rechoose(categories)
            {
                *categories = chosen;
            }
        }
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.conditions.is_empty() {
            return Err("A Select names at least one condition.".into());
        }
        self.conditions
            .iter()
            .try_for_each(SelectCondition::validate)
    }
    /// Every condition reads a node among `above`, of the kind it reads.
    pub(super) fn validate_against(&self, above: &[&CurationNode]) -> Result<(), String> {
        for condition in &self.conditions {
            let Some(node) = condition.node() else {
                continue;
            };
            if !above
                .iter()
                .any(|above| above.id() == node && condition.reads(above))
            {
                return Err(format!(
                    "A Select reads “{node}”, which is not a node above it of the kind it reads."
                ));
            }
        }
        Ok(())
    }
}

impl CurationGraph {
    /// The numbers a Select's episode condition may compare, in menu order:
    /// the recorded facts, then what each Track objects node above it
    /// counted.
    pub(crate) fn numbers_for(&self, select: &str) -> Vec<EpisodeNumber> {
        let mut numbers = EpisodeFact::ALL
            .into_iter()
            .map(EpisodeNumber::Fact)
            .collect::<Vec<_>>();
        for above in self.ancestors(select).into_iter().rev() {
            if let CurationNodeKind::TrackObjects(_) = above.kind() {
                numbers.push(EpisodeNumber::MostAtOnce(above.id().to_owned()));
                numbers.push(EpisodeNumber::DurationPer(above.id().to_owned()));
            }
        }
        numbers
    }
    /// Every condition a Select may add, built from the nodes above it, each
    /// from where it keeps the most: a number at least nothing, and a range
    /// at the threshold a review starts from.
    pub(crate) fn conditions_for(&self, select: &str) -> Vec<ConditionChoice> {
        let numbers = self.numbers_for(select).into_iter().map(|number| {
            SelectCondition::Episodes(EpisodeCondition::new(number, Comparison::AtLeast, 0.0))
        });
        let ranges = self
            .ancestors(select)
            .into_iter()
            .rev()
            .filter_map(|above| {
                let node = above.id().to_owned();
                Some(match above.kind() {
                    CurationNodeKind::ScoreProgress(_) => {
                        SelectCondition::Progressing { node, minimum: 0.0 }
                    }
                    CurationNodeKind::TrackObjects(_) => {
                        SelectCondition::Visible { node, fewest: 1 }
                    }
                    CurationNodeKind::FindSimilarSubtasks(_) => {
                        SelectCondition::Similar { node, minimum: 0.5 }
                    }
                    CurationNodeKind::GroupSimilarSubtasks(grouping) => {
                        SelectCondition::InCategories {
                            node,
                            categories: grouping
                                .categories()
                                .iter()
                                .map(|category| category.id())
                                .collect(),
                        }
                    }
                    CurationNodeKind::Dataset
                    | CurationNodeKind::BreakDownSubtasks(_)
                    | CurationNodeKind::Select(_) => return None,
                })
            })
            .collect::<Vec<_>>();
        numbers
            .chain(ranges)
            .map(|condition| ConditionChoice {
                label: condition.name(self),
                condition,
            })
            .collect()
    }
}

#[cfg(test)]
impl Selection {
    pub(crate) fn of(conditions: Vec<SelectCondition>) -> Self {
        Self { conditions }
    }
    /// Episodes whose one recorded fact compares with `value` as asked.
    pub(crate) fn episodes_where(fact: EpisodeFact, comparison: Comparison, value: f64) -> Self {
        Self::of(vec![SelectCondition::Episodes(EpisodeCondition::new(
            EpisodeNumber::Fact(fact),
            comparison,
            value,
        ))])
    }
}
