//! What each model node's preview has read this session: for every episode,
//! the rows that came back or why the read failed, under the column the node
//! stores its answers in.
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::CurationGraph;
use crate::activities::curation::measured_timeline::MeasuredTimeline;
use spiral_robotics_common::curation::AnswerColumn;
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;

/// What one episode's preview answered: its timeline, or its subtasks.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum PreviewRows {
    Timeline(MeasuredTimeline),
    Subtasks(Vec<SubtaskRow>),
}

#[derive(Clone, Debug, PartialEq)]
enum ReadOutcome {
    Rows(PreviewRows),
    Failed(String),
}

/// One node's reads, and the column they answer.
#[derive(Clone, Debug, PartialEq)]
struct NodeReads {
    column: AnswerColumn,
    episodes: BTreeMap<EpisodeId, ReadOutcome>,
}

/// Every node's reads, by node id.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct PreviewReads(BTreeMap<String, NodeReads>);

impl PreviewReads {
    /// Hold what `node`, storing its answers in `column`, answered for
    /// `episode`, replacing any earlier read of it. A read of another column
    /// replaces everything the node read before.
    pub(in crate::activities::curation) fn record(
        &mut self,
        (node, column): (&str, &AnswerColumn),
        episode: &EpisodeId,
        read: Result<PreviewRows, String>,
    ) {
        let fresh = || NodeReads {
            column: column.clone(),
            episodes: BTreeMap::new(),
        };
        let reads = self.0.entry(node.to_owned()).or_insert_with(fresh);
        if &reads.column != column {
            *reads = fresh();
        }
        let outcome = match read {
            Ok(rows) => ReadOutcome::Rows(rows),
            Err(reason) => ReadOutcome::Failed(reason),
        };
        reads.episodes.insert(episode.clone(), outcome);
    }
    /// `node`'s read of `episode`, when it answers the column it stores in
    /// now.
    fn read(
        &self,
        (node, column): (&str, &AnswerColumn),
        episode: &EpisodeId,
    ) -> Option<&ReadOutcome> {
        let reads = self.0.get(node)?;
        (&reads.column == column)
            .then(|| reads.episodes.get(episode))
            .flatten()
    }
    /// The timeline `node` read of `episode`; none until it has.
    pub(in crate::activities::curation) fn timeline(
        &self,
        node: (&str, &AnswerColumn),
        episode: &EpisodeId,
    ) -> Option<&MeasuredTimeline> {
        match self.read(node, episode) {
            Some(ReadOutcome::Rows(PreviewRows::Timeline(timeline))) => Some(timeline),
            _ => None,
        }
    }
    /// The subtasks `node` read of `episode`; none until it has.
    pub(in crate::activities::curation) fn subtasks(
        &self,
        node: (&str, &AnswerColumn),
        episode: &EpisodeId,
    ) -> &[SubtaskRow] {
        match self.read(node, episode) {
            Some(ReadOutcome::Rows(PreviewRows::Subtasks(rows))) => rows,
            _ => &[],
        }
    }
    /// Whether `node` has read `episode`, or tried.
    pub(super) fn has_read(&self, node: (&str, &AnswerColumn), episode: &EpisodeId) -> bool {
        self.read(node, episode).is_some()
    }
    /// Why `node`'s last read of `episode` failed, when it did.
    pub(super) fn failure(&self, node: (&str, &AnswerColumn), episode: &EpisodeId) -> Option<&str> {
        match self.read(node, episode) {
            Some(ReadOutcome::Failed(reason)) => Some(reason),
            _ => None,
        }
    }
    /// Let go of what nodes the graph no longer holds read.
    pub(in crate::activities::curation) fn forget_removed(&mut self, graph: &CurationGraph) {
        self.0.retain(|node, _| graph.node(node).is_some());
    }
}
