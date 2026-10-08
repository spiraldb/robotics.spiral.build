//! Where one model node's preview stands over the episodes that reach it.
use crate::activities::curation::population::PopulationReader;
use spiral_robotics_common::datasets::EpisodeId;

/// Which of the episodes reaching a node its preview has read, which failed,
/// and which it has not read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct PreviewProgress {
    read: Vec<EpisodeId>,
    failed: Vec<(EpisodeId, String)>,
    unread: Vec<EpisodeId>,
}

impl PreviewProgress {
    pub(in crate::activities::curation) fn of(reader: &PopulationReader<'_>, node: &str) -> Self {
        let Some(column) = reader.column(node) else {
            return Self::default();
        };
        let reads = reader.reads();
        let mut progress = Self::default();
        for episode in reader.input(node).episodes() {
            match (
                reads.has_read((node, column), &episode),
                reads.failure((node, column), &episode),
            ) {
                (false, _) => progress.unread.push(episode),
                (true, Some(reason)) => progress.failed.push((episode, reason.to_owned())),
                (true, None) => progress.read.push(episode),
            }
        }
        progress
    }
    /// The episodes read.
    pub(in crate::activities::curation) fn read(&self) -> &[EpisodeId] {
        &self.read
    }
    /// The episodes whose last read failed, with why.
    pub(in crate::activities::curation) fn failed(&self) -> &[(EpisodeId, String)] {
        &self.failed
    }
    /// The episodes a Run reads: those not read yet.
    pub(in crate::activities::curation) fn pending(&self) -> Vec<EpisodeId> {
        self.unread.clone()
    }
    /// Whether the preview has not read, or tried, any episode that reaches
    /// the node: a node that has nothing to show yet.
    pub(in crate::activities::curation) fn is_unmeasured(&self) -> bool {
        self.read.is_empty() && self.failed.is_empty()
    }
}
