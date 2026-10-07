//! What each node's preview has read this session: for every episode, the
//! rows that came back or why the read failed, and what went into it.
use super::input::InputPrint;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{CurationGraph, CurationNode, CurationNodeKind};
use crate::activities::curation::score_progress::Moment;
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;

/// What one episode's preview answered: its moments, its subtasks, or the
/// most objects it showed at once, which it lacks when the tracker could not
/// count them.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) enum PreviewRows {
    Moments(Vec<Moment>),
    Subtasks(Vec<SubtaskRow>),
    ObjectCount(Option<u64>),
}

/// What went into one read: the nearest node above as it was configured, and
/// the rows that reached this node from it. A read whose input is no longer
/// this is stale.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct ReadInput {
    upstream: CurationNodeKind,
    print: InputPrint,
}

impl ReadInput {
    pub(super) fn new(upstream: CurationNodeKind, print: InputPrint) -> Self {
        Self { upstream, print }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ReadOutcome {
    Rows(PreviewRows),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeRead {
    outcome: ReadOutcome,
    input: ReadInput,
}

/// One node's reads, and the configuration they answer.
#[derive(Clone, Debug, PartialEq)]
struct NodeReads {
    answers: CurationNodeKind,
    episodes: BTreeMap<EpisodeId, EpisodeRead>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct PreviewReads(BTreeMap<String, NodeReads>);

impl PreviewReads {
    /// Hold what `node`, configured as it is, answered for `episode` from
    /// `input`, replacing any earlier read of it. A read under another
    /// configuration replaces everything the node read before.
    pub(in crate::activities::curation) fn record(
        &mut self,
        node: &CurationNode,
        (episode, input): (&EpisodeId, ReadInput),
        read: Result<PreviewRows, String>,
    ) {
        let fresh = || NodeReads {
            answers: node.kind().clone(),
            episodes: BTreeMap::new(),
        };
        let reads = self.0.entry(node.id().to_owned()).or_insert_with(fresh);
        if &reads.answers != node.kind() {
            *reads = fresh();
        }
        let outcome = match read {
            Ok(rows) => ReadOutcome::Rows(rows),
            Err(reason) => ReadOutcome::Failed(reason),
        };
        reads
            .episodes
            .insert(episode.clone(), EpisodeRead { outcome, input });
    }
    /// `node`'s read of `episode`, when it answers the node as it is now.
    fn read(&self, node: &CurationNode, episode: &EpisodeId) -> Option<&EpisodeRead> {
        let reads = self.0.get(node.id())?;
        (&reads.answers == node.kind())
            .then(|| reads.episodes.get(episode))
            .flatten()
    }
    /// The moments `node` read for `episode`; none until it has.
    pub(in crate::activities::curation) fn moments(
        &self,
        node: &CurationNode,
        episode: &EpisodeId,
    ) -> &[Moment] {
        match self.read(node, episode).map(|read| &read.outcome) {
            Some(ReadOutcome::Rows(PreviewRows::Moments(moments))) => moments,
            _ => &[],
        }
    }
    /// The subtasks `node` read for `episode`; none until it has.
    pub(in crate::activities::curation) fn subtasks(
        &self,
        node: &CurationNode,
        episode: &EpisodeId,
    ) -> &[SubtaskRow] {
        match self.read(node, episode).map(|read| &read.outcome) {
            Some(ReadOutcome::Rows(PreviewRows::Subtasks(rows))) => rows,
            _ => &[],
        }
    }
    /// The most objects `node` saw at once in `episode`; none until it has
    /// read the episode, or when the tracker could not count it.
    pub(in crate::activities::curation) fn object_count(
        &self,
        node: &CurationNode,
        episode: &EpisodeId,
    ) -> Option<u64> {
        match self.read(node, episode).map(|read| &read.outcome) {
            Some(ReadOutcome::Rows(PreviewRows::ObjectCount(count))) => *count,
            _ => None,
        }
    }
    /// What went into `node`'s read of `episode`, when it has one.
    pub(super) fn input(&self, node: &CurationNode, episode: &EpisodeId) -> Option<&ReadInput> {
        self.read(node, episode).map(|read| &read.input)
    }
    /// Whether any read `node` holds was made below another configuration of
    /// `upstream`'s node than it has now.
    pub(super) fn read_below_other(
        &self,
        node: &CurationNode,
        upstream: &CurationNodeKind,
    ) -> bool {
        self.0.get(node.id()).is_some_and(|reads| {
            &reads.answers == node.kind()
                && reads
                    .episodes
                    .values()
                    .any(|read| &read.input.upstream != upstream)
        })
    }
    /// Why `node`'s last read of `episode` failed, when it did.
    pub(super) fn failure(&self, node: &CurationNode, episode: &EpisodeId) -> Option<&str> {
        match self.read(node, episode).map(|read| &read.outcome) {
            Some(ReadOutcome::Failed(reason)) => Some(reason),
            _ => None,
        }
    }
    /// Let go of what nodes the graph no longer holds read.
    pub(in crate::activities::curation) fn forget_removed(&mut self, graph: &CurationGraph) {
        self.0.retain(|node, _| graph.node(node).is_some());
    }
}

#[cfg(test)]
impl ReadInput {
    /// An input nothing is compared with, for tests of what a read holds
    /// rather than of whether it is current.
    pub(in crate::activities::curation) fn untracked() -> Self {
        Self::new(CurationNodeKind::Dataset, InputPrint::untracked())
    }
}
