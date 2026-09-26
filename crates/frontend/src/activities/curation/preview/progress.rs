//! Where one measurement's preview stands over the episodes that reach it.
use super::input::PreviewInput;
use super::reads::ReadInput;
use crate::activities::curation::document::CurationNodeKind;
use crate::activities::curation::population::PopulationReader;
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;

/// Which of the episodes reaching a node its preview has read from the rows
/// that reach it now, which it read from rows that have changed since, which
/// failed, and which it has not read. A node is stale when it has read from
/// rows that changed, or when the measurement above it is stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct PreviewProgress {
    read: Vec<EpisodeId>,
    stale: Vec<EpisodeId>,
    failed: Vec<(EpisodeId, String)>,
    unread: Vec<EpisodeId>,
    stale_above: bool,
}

impl PreviewProgress {
    pub(in crate::activities::curation) fn of(reader: &PopulationReader<'_>, node: &str) -> Self {
        let graph = reader.graph();
        let (Some(measured), Some(above)) = (graph.node(node), graph.measure_above(node)) else {
            return Self::default();
        };
        let mut current = Self::inputs(reader, node);
        // What is above changed: its configuration since this node ran, or
        // what reaches it since it ran.
        let mut progress = Self {
            stale_above: reader.reads().read_below_other(measured, above.kind())
                || (!matches!(above.kind(), CurationNodeKind::Dataset)
                    && Self::of(reader, above.id()).is_stale()),
            ..Self::default()
        };
        for episode in reader.input(node).episodes() {
            let current = current.remove(&episode);
            let reads = reader.reads();
            match (
                reads.input(measured, &episode),
                reads.failure(measured, &episode),
            ) {
                (None, _) => progress.unread.push(episode),
                (Some(input), _) if Some(input) != current.as_ref() => {
                    progress.stale.push(episode);
                }
                (Some(_), Some(reason)) => progress.failed.push((episode, reason.to_owned())),
                (Some(_), None) => progress.read.push(episode),
            }
        }
        progress
    }
    /// What would go into `node`'s read of each episode that reaches it now:
    /// the nearest node above as it is configured, and the rows it hands on.
    fn inputs(reader: &PopulationReader<'_>, node: &str) -> BTreeMap<EpisodeId, ReadInput> {
        let graph = reader.graph();
        let Some(above) = graph.measure_above(node) else {
            return BTreeMap::new();
        };
        PreviewInput::of(graph, node)
            .ok()
            .and_then(|shape| shape.rows(&reader.input(node), reader.sample()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(episode, rows)| (episode, ReadInput::new(above.kind().clone(), rows.print())))
            .collect()
    }
    /// The episodes read from the rows that reach them now.
    pub(in crate::activities::curation) fn read(&self) -> &[EpisodeId] {
        &self.read
    }
    /// The episodes whose last read failed from the rows that reach them now,
    /// with why.
    pub(in crate::activities::curation) fn failed(&self) -> &[(EpisodeId, String)] {
        &self.failed
    }
    /// The episodes a Run reads: those not read yet, and those read from
    /// rows that have changed since.
    pub(in crate::activities::curation) fn pending(&self) -> Vec<EpisodeId> {
        self.unread.iter().chain(&self.stale).cloned().collect()
    }
    pub(in crate::activities::curation) fn is_stale(&self) -> bool {
        self.stale_above || !self.stale.is_empty()
    }
    /// Whether the preview has not read, tried, or once held any episode that
    /// reaches the node: a measurement that has nothing to show yet.
    pub(in crate::activities::curation) fn is_unmeasured(&self) -> bool {
        self.read.is_empty() && self.stale.is_empty() && self.failed.is_empty()
    }
}

#[cfg(test)]
impl ReadInput {
    /// What a run of `node` would record went into its read of `episode`.
    pub(in crate::activities::curation) fn current(
        reader: &PopulationReader<'_>,
        node: &str,
        episode: &EpisodeId,
    ) -> Self {
        PreviewProgress::inputs(reader, node)
            .remove(episode)
            .expect("the episode reaches the node")
    }
}
