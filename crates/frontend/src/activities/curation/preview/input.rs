//! What reaches a node in its preview, written out for Spiral as `VALUES`:
//! the rows its parent's preview holds, in the columns the published view
//! would hand the node.
use crate::activities::curation::annotate::SubtaskColumns;
use crate::activities::curation::document::{CurationGraph, CurationNodeKind, Selection};
use crate::activities::curation::path::{CurationPath, PathError, Relation, StepInput};
use crate::activities::curation::population::{EpisodeSample, Population, Slice};
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// The `WITH` entry a preview's input rows are held in.
const INPUT: &str = "preview_input";

/// A fingerprint of the rows one episode gave a node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::activities::curation) struct InputPrint(u64);

/// One episode's input rows as a statement lists them, and their print.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeRows {
    rows: Vec<String>,
    print: InputPrint,
}

impl EpisodeRows {
    fn new(rows: Vec<String>) -> Self {
        let mut hasher = DefaultHasher::new();
        rows.hash(&mut hasher);
        Self {
            print: InputPrint(hasher.finish()),
            rows,
        }
    }
    pub(super) fn rows(&self) -> &[String] {
        &self.rows
    }
    pub(super) fn print(&self) -> InputPrint {
        self.print
    }
}

/// How one node's preview is given what reaches it.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PreviewInput {
    /// The intervals that reach a node that reads each episode's recording,
    /// one episode bound as `$1` at a time. After a Select of moments these
    /// are the stretches it joins the kept moments into, as its step does.
    Intervals { stretches: bool },
    /// Every column of the subtasks that reach a text judgment, many
    /// episodes at once.
    Subtasks {
        relation: Relation,
        columns: SubtaskColumns,
    },
}

impl PreviewInput {
    pub(super) fn of(graph: &CurationGraph, node: &str) -> Result<Self, PathError> {
        let kind = graph
            .node(node)
            .map(|node| node.kind())
            .ok_or_else(|| PathError::new("That node is no longer in the graph."))?;
        let parent = graph
            .parent_of(node)
            .ok_or_else(|| PathError::new("Only the dataset starts a path."))?;
        match kind {
            CurationNodeKind::IdentifyUnproductiveMoments(_)
            | CurationNodeKind::BreakDownSubtasks(_) => Ok(Self::Intervals {
                stretches: matches!(
                    graph.node(parent).map(|parent| parent.kind()),
                    Some(CurationNodeKind::Select(Selection::Progressing { .. }))
                ),
            }),
            CurationNodeKind::FindSimilarSubtasks(_)
            | CurationNodeKind::GroupSimilarSubtasks(_) => {
                let relation = CurationPath::to(graph, parent)?.relation()?.renamed(INPUT);
                let columns = SubtaskColumns::of(&relation).map_err(PathError::new)?;
                Ok(Self::Subtasks { relation, columns })
            }
            CurationNodeKind::Dataset | CurationNodeKind::Select(_) => {
                Err(PathError::new("Only a measurement previews."))
            }
        }
    }

    /// Each reaching episode's rows as this node's input writes them.
    pub(super) fn rows(
        &self,
        input: &Population,
        sample: &EpisodeSample,
    ) -> Result<BTreeMap<EpisodeId, EpisodeRows>, String> {
        let origin = |episode: &EpisodeId| {
            sample
                .origin_ns(episode)
                .ok_or_else(|| format!("Episode {episode} has no recorded start time."))
        };
        let mut rows = BTreeMap::<EpisodeId, Vec<String>>::new();
        match self {
            Self::Intervals { stretches } => {
                let slices = if *stretches {
                    input.slices(sample)
                } else {
                    input.items(sample)
                };
                for slice in slices.iter().filter(|slice| slice.seconds() > 0.0) {
                    let row = Self::interval(slice, origin(slice.episode())?);
                    rows.entry(slice.episode().clone()).or_default().push(row);
                }
            }
            Self::Subtasks { columns, .. } => {
                for row in input.subtasks() {
                    let values = columns.values(row, origin(row.episode())?)?;
                    rows.entry(row.episode().clone()).or_default().push(values);
                }
            }
        }
        Ok(rows
            .into_iter()
            .map(|(episode, rows)| (episode, EpisodeRows::new(rows)))
            .collect())
    }

    /// One interval of the bound episode, in the recording's clock.
    fn interval(slice: &Slice, origin_ns: i64) -> String {
        let nanos = |seconds: f64| origin_ns + (seconds * 1e9).round() as i64;
        format!(
            "($1, CAST({} AS BIGINT), CAST({} AS BIGINT))",
            nanos(slice.start()),
            nanos(slice.end())
        )
    }

    /// The `WITH` entries holding `rows`, and the relation they make.
    pub(super) fn step_input<'a>(&self, rows: impl Iterator<Item = &'a String>) -> StepInput {
        let values = rows.map(String::as_str).collect::<Vec<_>>().join(", ");
        match self {
            Self::Intervals { .. } => StepInput::new(
                vec![format!(
                    "{INPUT}(episode_id, start_ns, end_ns) AS (VALUES {values})"
                )],
                Relation::new(INPUT, Vec::new()),
            ),
            Self::Subtasks { relation, columns } => StepInput::new(
                vec![
                    format!(
                        "{INPUT}_values({}) AS (VALUES {values})",
                        columns.value_names().join(", ")
                    ),
                    format!(
                        "{INPUT} AS (SELECT {} FROM {INPUT}_values)",
                        columns.rebuilt().join(", ")
                    ),
                ],
                relation.clone(),
            ),
        }
    }
}

#[cfg(test)]
impl InputPrint {
    pub(super) fn untracked() -> Self {
        Self(0)
    }
}
