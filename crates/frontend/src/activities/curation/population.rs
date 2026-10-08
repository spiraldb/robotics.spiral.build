//! What flows along an edge, over the sample: which episodes reach a node,
//! and which ranges of each are kept.
//!
//! A curation is shaped against an interpretive sample — the first hundred
//! episodes its dataset lists — so every node answers while the graph is
//! still being built. The dataset hands on those episodes whole; a model
//! node hands on what reaches it and adds what its preview read; and a
//! Select keeps part of it by its conditions (`select/reading.rs`), as the
//! published view's SQL does.
use super::annotate::SubtaskRow;
use super::document::{CurationGraph, CurationNodeKind};
use super::measured_timeline::MeasuredTimeline;
use super::path::CurationPath;
use super::preview::PreviewReads;
use crate::activities::episode::Episode;
use spiral_robotics_common::curation::AnswerColumn;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::BTreeMap;

/// How many of a dataset's episodes interpretive execution reads.
pub(super) const INTERPRETED_EPISODES: usize = 100;

/// The episodes a curation is shaped against: the first the dataset lists,
/// in recording order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeSample {
    /// Which dataset's episodes these are, so a sample still held from the
    /// dataset before is never read as this one's.
    dataset: DatasetSelection,
    episodes: Vec<Episode>,
    /// Whether the dataset lists more episodes than the sample holds.
    partial: bool,
}

impl EpisodeSample {
    pub(super) async fn load(dataset: &DatasetSelection) -> Result<Self, String> {
        let page = Episode::page(dataset, 0, INTERPRETED_EPISODES).await?;
        Ok(Self {
            dataset: dataset.clone(),
            partial: page.has_more(),
            episodes: page.into_episodes(),
        })
    }
    pub(super) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(super) fn episodes(&self) -> &[Episode] {
        &self.episodes
    }
    pub(super) fn episode(&self, id: &EpisodeId) -> Option<&Episode> {
        self.episodes.iter().find(|episode| episode.id() == id)
    }
    /// Seconds from an episode's first recorded message to its last.
    pub(super) fn duration(&self, id: &EpisodeId) -> Option<f64> {
        self.episode(id)?.facts().duration_seconds()
    }
    /// The log time of an episode's first message, which every time the
    /// browser shows is counted from.
    pub(super) fn origin_ns(&self, id: &EpisodeId) -> Option<i64> {
        let start = self.episode(id)?.facts().message_start_time()?;
        i64::try_from(start).ok()
    }
    pub(super) fn is_partial(&self) -> bool {
        self.partial
    }
    #[cfg(test)]
    pub(super) fn fixture(episodes: Vec<Episode>) -> Self {
        Self {
            dataset: episodes.first().map_or_else(
                || DatasetSelection::new(vec!["fixture".to_owned()]).expect("a dataset name"),
                |episode| episode.dataset().clone(),
            ),
            episodes,
            partial: false,
        }
    }
}

/// The ranges kept of one episode, in seconds from its first message, in
/// time order, never overlapping or touching.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct KeptRanges(Vec<(f64, f64)>);

impl KeptRanges {
    /// `ranges` in time order, those that overlap or touch joined into one,
    /// as the SQL folds them: two subtasks back to back are one stretch.
    fn merged(mut ranges: Vec<(f64, f64)>) -> Self {
        ranges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut merged: Vec<(f64, f64)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        Self(merged)
    }
    /// The whole of an episode `seconds` long.
    pub(super) fn whole(seconds: f64) -> Self {
        Self(vec![(0.0, seconds)])
    }
    pub(super) fn ranges(&self) -> &[(f64, f64)] {
        &self.0
    }
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(super) fn seconds(&self) -> f64 {
        self.0
            .iter()
            .map(|(start, end)| (end - start).max(0.0))
            .sum()
    }
    /// What is kept of these within `within`, a range at a time, as the SQL
    /// intersects them: every overlap of one of these with one of those,
    /// merged.
    pub(super) fn within(&self, within: &[(f64, f64)]) -> Self {
        Self::merged(
            self.0
                .iter()
                .flat_map(|(start, end)| {
                    within
                        .iter()
                        .filter(move |(from, to)| from < end && to > start)
                        .map(move |(from, to)| (start.max(*from), end.min(*to)))
                })
                .collect(),
        )
    }
    /// What of these `other` does not keep: the parts left out between them.
    pub(super) fn without(&self, other: &Self) -> Self {
        let mut left = Vec::new();
        for (start, end) in &self.0 {
            let mut from = *start;
            for (kept_start, kept_end) in &other.0 {
                if *kept_end <= from || *kept_start >= *end {
                    continue;
                }
                if *kept_start > from {
                    left.push((from, *kept_start));
                }
                from = from.max(*kept_end);
            }
            if from < *end {
                left.push((from, *end));
            }
        }
        Self(left)
    }
    /// Whether the instant `time` is kept.
    pub(super) fn holds(&self, time: f64) -> bool {
        self.0
            .iter()
            .any(|(start, end)| *start <= time && time < *end)
    }
}

/// One episode a node hands on, and what of it is kept.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct KeptEpisode {
    episode: EpisodeId,
    kept: KeptRanges,
}

impl KeptEpisode {
    pub(super) fn new(episode: EpisodeId, kept: KeptRanges) -> Self {
        Self { episode, kept }
    }
    pub(super) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(super) fn kept(&self) -> &KeptRanges {
        &self.kept
    }
}

/// What reaches a node, or what it hands on: the episodes it keeps any of,
/// in sample order, each with its kept ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Population(Vec<KeptEpisode>);

impl Population {
    pub(super) fn new(episodes: Vec<KeptEpisode>) -> Self {
        Self(episodes)
    }
    pub(super) fn kept_episodes(&self) -> &[KeptEpisode] {
        &self.0
    }
    /// The episodes it reaches.
    pub(super) fn episodes(&self) -> Vec<EpisodeId> {
        self.0.iter().map(|kept| kept.episode.clone()).collect()
    }
    /// What it keeps of `episode`, when it reaches it.
    pub(super) fn of(&self, episode: &EpisodeId) -> Option<&KeptRanges> {
        self.0
            .iter()
            .find(|kept| &kept.episode == episode)
            .map(|kept| &kept.kept)
    }
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
    /// How much recording it keeps.
    pub(super) fn seconds(&self) -> f64 {
        self.0.iter().map(|kept| kept.kept.seconds()).sum()
    }
}

/// One part of one episode, in seconds from its start, `start` inclusive and
/// `end` exclusive: what a map draws and a listing plays. A subtask's slice
/// also carries the words the annotation described it with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Slice {
    episode: EpisodeId,
    start: f64,
    end: f64,
    label: Option<String>,
}

impl Slice {
    pub(super) fn new(episode: EpisodeId, start: f64, end: f64, label: Option<String>) -> Self {
        Self {
            episode,
            start,
            end,
            label,
        }
    }
    /// One subtask row's interval, under the words it was described with.
    pub(super) fn of_subtask(row: &SubtaskRow) -> Self {
        Self::new(
            row.episode().clone(),
            row.start_time(),
            row.end_time(),
            Some(row.description().to_owned()),
        )
    }
    pub(super) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(super) fn start(&self) -> f64 {
        self.start
    }
    pub(super) fn end(&self) -> f64 {
        self.end
    }
    pub(super) fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

/// Reads what each node of one graph hands on over one sample, and what the
/// previews of its model nodes have read over it, each under the column the
/// node stores its answers in now.
pub(super) struct PopulationReader<'a> {
    graph: &'a CurationGraph,
    sample: &'a EpisodeSample,
    reads: &'a PreviewReads,
    columns: BTreeMap<String, AnswerColumn>,
}

impl<'a> PopulationReader<'a> {
    pub(super) fn new(
        graph: &'a CurationGraph,
        sample: &'a EpisodeSample,
        reads: &'a PreviewReads,
    ) -> Self {
        let columns = graph
            .nodes()
            .iter()
            .filter(|node| node.kind().is_model())
            .filter_map(|node| {
                let column = CurationPath::to(graph, node.id())
                    .and_then(|path| path.answer_column())
                    .ok()?;
                Some((node.id().to_owned(), column))
            })
            .collect();
        Self {
            graph,
            sample,
            reads,
            columns,
        }
    }
    pub(super) fn graph(&self) -> &CurationGraph {
        self.graph
    }
    pub(super) fn sample(&self) -> &EpisodeSample {
        self.sample
    }
    pub(super) fn reads(&self) -> &PreviewReads {
        self.reads
    }
    /// The column `node` stores its answers in now, once it can ask.
    pub(super) fn column(&self, node: &str) -> Option<&AnswerColumn> {
        self.columns.get(node)
    }
    /// The timeline `node`'s preview read of `episode`, when it has one.
    pub(super) fn timeline(&self, node: &str, episode: &EpisodeId) -> Option<&MeasuredTimeline> {
        self.reads.timeline((node, self.column(node)?), episode)
    }
    /// The subtasks `node`'s preview read of `episode`; none until it has.
    pub(super) fn subtasks(&self, node: &str, episode: &EpisodeId) -> &[SubtaskRow] {
        match self.column(node) {
            Some(column) => self.reads.subtasks((node, column), episode),
            None => &[],
        }
    }
    /// The subtasks `node`'s preview read of every episode that reaches it,
    /// in sample order.
    pub(super) fn subtasks_reaching(&self, node: &str) -> Vec<SubtaskRow> {
        self.input(node)
            .episodes()
            .iter()
            .flat_map(|episode| self.subtasks(node, episode))
            .cloned()
            .collect()
    }
    /// What `node` hands on. A node the graph does not hold hands on
    /// nothing.
    pub(super) fn of(&self, node: &str) -> Population {
        let Some(found) = self.graph.node(node) else {
            return Population::default();
        };
        match found.kind() {
            CurationNodeKind::Dataset => Population::new(
                self.sample
                    .episodes()
                    .iter()
                    .map(|episode| {
                        let seconds = episode.facts().duration_seconds().unwrap_or_default();
                        KeptEpisode::new(episode.id().clone(), KeptRanges::whole(seconds))
                    })
                    .collect(),
            ),
            CurationNodeKind::Select(selection) => selection.keep(&self.input(node), self),
            _ => self.input(node),
        }
    }
    /// What reaches `node`: what its parent hands on.
    pub(super) fn input(&self, node: &str) -> Population {
        match self.graph.parent_of(node) {
            Some(parent) => self.of(parent),
            None => Population::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_ranges_narrow_as_the_sql_intersects_them_and_say_what_they_leave_out() {
        let kept = KeptRanges(vec![(0.0, 15.0), (25.0, 50.0)]);
        let within = kept.within(&[(0.0, 20.0), (30.0, 50.0)]);
        assert_eq!(within, KeptRanges(vec![(0.0, 15.0), (30.0, 50.0)]));
        assert_eq!(within.seconds(), 35.0);
        assert_eq!(kept.without(&within), KeptRanges(vec![(25.0, 30.0)]));
        assert!(within.holds(31.0) && !within.holds(20.0));
        assert!(kept.within(&[]).is_empty());
    }
}
