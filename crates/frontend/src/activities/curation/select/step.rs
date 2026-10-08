//! A Select's step in a path: its conditions as SQL over the reach entry
//! above it, ending in its own reach entry, one row per episode it keeps
//! with its narrowed `kept`.
//!
//! Episode conditions read each reaching episode's numbers beside its row —
//! the dataset's facts, and what each Track objects node's entries count —
//! rank them when a condition ranks, and keep the rows that pass. Range
//! conditions unnest `kept`, intersect it with the ranges each condition
//! keeps, read from the entries of the node it names, and fold what is left
//! back per episode, ranges that overlap or touch merged: an episode with
//! nothing left is gone. Each step reads
//! the reach entry above it once.
use crate::activities::curation::document::{
    Comparison, CurationGraph, CurationNode, CurationNodeKind, EpisodeCondition, EpisodeNumber,
    SelectCondition, Selection,
};
use crate::activities::curation::graph::DatasetStep;
use crate::activities::curation::path::PathError;
use spiral_robotics_common::sql::SqlLiteral;

/// Where a Select's step is written: the name its reach entry takes, the
/// reach entry above it, and the graph naming the nodes it reads.
pub(in crate::activities::curation) struct SelectStep<'a> {
    name: String,
    input: &'a str,
    graph: &'a CurationGraph,
}

impl<'a> SelectStep<'a> {
    pub(in crate::activities::curation) fn new(
        node: &CurationNode,
        input: &'a str,
        graph: &'a CurationGraph,
    ) -> Self {
        Self {
            name: node.stem(),
            input,
            graph,
        }
    }
    /// The entries of the node `id`, which a condition reads.
    fn entries_of(&self, id: &str) -> Result<String, PathError> {
        self.graph
            .node(id)
            .map(|node| format!("{}_entries", node.stem()))
            .ok_or_else(|| PathError::new("A Select reads a node that is no longer in the graph."))
    }
}

impl Selection {
    /// The step's entries, the last named after it: the rows of `step`'s
    /// input meeting every episode condition, with `kept` narrowed by every
    /// range condition.
    pub(in crate::activities::curation) fn step(
        &self,
        step: &SelectStep<'_>,
    ) -> Result<Vec<String>, PathError> {
        let numbered = self
            .conditions()
            .iter()
            .enumerate()
            .filter_map(|(index, condition)| match condition {
                SelectCondition::Episodes(condition) => Some((index, condition)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let ranges = self
            .conditions()
            .iter()
            .enumerate()
            .filter(|(_, condition)| condition.narrows())
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        let passing = if numbered.is_empty() {
            step.input.to_owned()
        } else {
            let passing = if ranges.is_empty() {
                step.name.clone()
            } else {
                format!("{}_passing", step.name)
            };
            entries.extend(EpisodeConditions(&numbered).entries(step, &passing)?);
            passing
        };
        if ranges.is_empty() {
            return Ok(entries);
        }
        let name = &step.name;
        let mut kept = format!("{name}_unnested");
        entries.push(format!(
            "{kept} AS (SELECT p.episode_id, k.start_ns, k.end_ns FROM {passing} p CROSS JOIN LATERAL UNNEST(p.kept) k)"
        ));
        for (index, condition) in ranges {
            let within = format!("{name}_within_{index}");
            entries.extend(condition.within(&within, step)?);
            let narrowed = format!("{name}_kept_{index}");
            entries.push(format!(
                "{narrowed} AS (SELECT k.episode_id, GREATEST(k.start_ns, w.start_ns) AS start_ns, \
                 LEAST(k.end_ns, w.end_ns) AS end_ns FROM {kept} k JOIN {within} w \
                 ON w.episode_id = k.episode_id AND w.start_ns < k.end_ns AND w.end_ns > k.start_ns)"
            ));
            kept = narrowed;
        }
        // Ranges that overlap or touch are one: a range opens a new island
        // unless it starts by the latest end before it.
        let order = "PARTITION BY episode_id ORDER BY start_ns, end_ns";
        entries.push(format!(
            "{name}_islands AS (SELECT episode_id, start_ns, end_ns, SUM(opens) OVER ({order} ROWS UNBOUNDED PRECEDING) AS island \
             FROM (SELECT episode_id, start_ns, end_ns, CASE WHEN start_ns <= MAX(end_ns) OVER \
             ({order} ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) THEN 0 ELSE 1 END AS opens FROM {kept}) flagged)"
        ));
        entries.push(format!(
            "{name} AS (SELECT episode_id, spql.list.collect(STRUCT_PACK(start_ns := start_ns, end_ns := end_ns)) AS kept \
             FROM (SELECT episode_id, MIN(start_ns) AS start_ns, MAX(end_ns) AS end_ns FROM {name}_islands \
             GROUP BY episode_id, island ORDER BY episode_id, start_ns) ordered GROUP BY episode_id)"
        ));
        Ok(entries)
    }
}

/// A Select's episode conditions, each by its place among the Select's
/// conditions, which names its number (`number_2`) and rank (`rank_2`).
struct EpisodeConditions<'a>(&'a [(usize, &'a EpisodeCondition)]);

impl EpisodeConditions<'_> {
    /// The rows of the step's input meeting every condition, as the entry
    /// `passing`: each row's numbers beside it, ranked when a condition
    /// ranks.
    fn entries(&self, step: &SelectStep<'_>, passing: &str) -> Result<Vec<String>, PathError> {
        let name = &step.name;
        let mut joins = format!(
            " JOIN {} d ON d.episode_id = r.episode_id",
            DatasetStep::NAME
        );
        let mut numbers = Vec::new();
        for (index, condition) in self.0 {
            let most = |joins: &mut String, node: &str| -> Result<String, PathError> {
                joins.push_str(&format!(
                    " LEFT JOIN (SELECT episode_id, MAX(spql.list.len(objects)) AS most FROM {} GROUP BY episode_id) m{index} \
                     ON m{index}.episode_id = r.episode_id",
                    step.entries_of(node)?
                ));
                Ok(format!("m{index}.most"))
            };
            let number = match condition.number() {
                EpisodeNumber::Fact(fact) => format!("d.{}", fact.key()),
                EpisodeNumber::MostAtOnce(node) => most(&mut joins, node)?,
                EpisodeNumber::DurationPer(node) => {
                    let most = most(&mut joins, node)?;
                    format!(
                        "CASE WHEN {most} > 0 THEN d.duration_seconds / CAST({most} AS DOUBLE) END"
                    )
                }
            };
            numbers.push(format!("{number} AS number_{index}"));
        }
        let mut entries = vec![format!(
            "{name}_numbers AS (SELECT r.episode_id, r.kept, {} FROM {} r{joins})",
            numbers.join(", "),
            step.input
        )];
        let ranks = self
            .0
            .iter()
            .filter(|(_, condition)| condition.comparison().ranks())
            .map(|(index, condition)| Self::rank(*index, condition.comparison()))
            .collect::<Vec<_>>();
        let judged = if ranks.is_empty() {
            format!("{name}_numbers")
        } else {
            let named = (0..self.0.len())
                .map(|place| format!("number_{}", self.0[place].0))
                .collect::<Vec<_>>()
                .join(", ");
            entries.push(format!(
                "{name}_ranked AS (SELECT episode_id, kept, {named}, {} FROM {name}_numbers)",
                ranks.join(", ")
            ));
            format!("{name}_ranked")
        };
        let predicates = self
            .0
            .iter()
            .map(|(index, condition)| Self::predicate(*index, condition))
            .collect::<Result<Vec<_>, _>>()?;
        entries.push(format!(
            "{passing} AS (SELECT episode_id, kept FROM {judged} WHERE {})",
            predicates.join(" AND ")
        ));
        Ok(entries)
    }

    /// The percent rank of number `index` among the rows that have one:
    /// its rank from the end `comparison` keeps, from 0, over their count
    /// less one. Spiral has no `PERCENT_RANK`, so it is spelled with `RANK`
    /// and `COUNT`.
    fn rank(index: usize, comparison: Comparison) -> String {
        let number = format!("number_{index}");
        let order = match comparison {
            Comparison::Highest => " DESC",
            _ => "",
        };
        let rows = format!("PARTITION BY {number} IS NULL");
        format!(
            "CAST(RANK() OVER ({rows} ORDER BY {number}{order}) - 1 AS DOUBLE) / \
             CAST(GREATEST(COUNT({number}) OVER ({rows}), 2) - 1 AS DOUBLE) AS rank_{index}"
        )
    }

    /// What condition `index` keeps, in SQL: an episode without the number
    /// compares as unknown, so it is not kept.
    fn predicate(index: usize, condition: &EpisodeCondition) -> Result<String, PathError> {
        let value = SqlLiteral::double(condition.value())
            .ok_or_else(|| PathError::new("A Select's threshold must be a number."))?;
        Ok(match condition.comparison() {
            Comparison::AtLeast => format!("number_{index} >= {value}"),
            Comparison::AtMost => format!("number_{index} <= {value}"),
            Comparison::Lowest | Comparison::Highest => {
                format!("number_{index} IS NOT NULL AND rank_{index} <= {value}")
            }
        })
    }
}

impl SelectCondition {
    /// The entries holding the ranges this range condition keeps of each
    /// episode, the last named `within`.
    fn within(&self, within: &str, step: &SelectStep<'_>) -> Result<Vec<String>, PathError> {
        let literal = |value: f64| {
            SqlLiteral::double(value).ok_or_else(|| PathError::new("A threshold must be a number."))
        };
        Ok(match self {
            Self::Episodes(_) => Vec::new(),
            Self::Progressing { node, minimum } => TimelineStretches {
                within,
                entries: &step.entries_of(node)?,
                value: "progress_rate",
            }
            .entries(&format!(
                "(value IS NULL OR value >= {})",
                literal(*minimum)?
            )),
            Self::Visible { node, fewest } => TimelineStretches {
                within,
                entries: &step.entries_of(node)?,
                value: "spql.list.len(objects)",
            }
            .entries(&format!("value >= {fewest}")),
            Self::Similar { node, minimum } => vec![format!(
                "{within} AS (SELECT episode_id, start_ns, end_ns FROM {} WHERE similarity >= {})",
                step.entries_of(node)?,
                literal(*minimum)?
            )],
            Self::InCategories { node, categories } => {
                let Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) =
                    step.graph.node(node).map(CurationNode::kind)
                else {
                    return Err(PathError::new("A Select reads a grouping that is gone."));
                };
                let names = categories
                    .iter()
                    .filter_map(|id| grouping.category(*id))
                    .map(|category| SqlLiteral::text(category.name()))
                    .collect::<Vec<_>>();
                if names.is_empty() {
                    return Err(PathError::new(
                        "Choose at least one category: this Select keeps none.",
                    ));
                }
                vec![format!(
                    "{within} AS (SELECT episode_id, start_ns, end_ns FROM {} WHERE category IN ({}))",
                    step.entries_of(node)?,
                    names.join(", ")
                )]
            }
        })
    }
}

/// The stretches of a timeline's moments that pass: each entry's moment from
/// its time to the next one's, the last lasting as long as the one before it
/// and a lone entry none, joined where passing moments follow each other.
/// Stretches are found by the difference of two `ROW_NUMBER`s, the moments
/// and the window over them being separate entries.
struct TimelineStretches<'a> {
    within: &'a str,
    entries: &'a str,
    value: &'a str,
}

impl TimelineStretches<'_> {
    fn entries(&self, passes: &str) -> Vec<String> {
        let (within, entries, value) = (self.within, self.entries, self.value);
        let moments = "OVER (PARTITION BY episode_id ORDER BY timestamp_ns)";
        vec![
            format!(
                "{within}_moments AS (SELECT episode_id, timestamp_ns AS start_ns, \
                 COALESCE(LEAD(timestamp_ns) {moments}, timestamp_ns + (timestamp_ns - LAG(timestamp_ns) {moments})) AS end_ns, \
                 {value} AS value, ROW_NUMBER() {moments} AS position FROM {entries})"
            ),
            format!(
                "{within}_passing AS (SELECT episode_id, start_ns, end_ns, \
                 position - ROW_NUMBER() OVER (PARTITION BY episode_id ORDER BY start_ns) AS stretch \
                 FROM {within}_moments WHERE end_ns > start_ns AND {passes})"
            ),
            format!(
                "{within} AS (SELECT episode_id, MIN(start_ns) AS start_ns, MAX(end_ns) AS end_ns \
                 FROM {within}_passing GROUP BY episode_id, stretch)"
            ),
        ]
    }
}
