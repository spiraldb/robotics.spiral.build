//! What flows along an edge: which parts of which episodes a node hands on.
//!
//! A curation is shaped against an interpretive sample — the first hundred
//! episodes its dataset lists — so every node answers while the graph is
//! still being built. The dataset hands on those episodes whole; a
//! measurement hands on what its preview read over the episodes it received,
//! already cut to what reached it; and a node's rule, a Select's or a
//! measure's own, keeps part of that.
//! Any population reads out as slices, one part of one episode each, which is
//! what a published view holds one row per.
use super::annotate::SubtaskRow;
use super::document::{CurationGraph, CurationNode, CurationNodeKind, Selection};
use super::preview::PreviewReads;
use super::score_progress::Moment;
use crate::activities::episode::Episode;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

/// How many of a dataset's episodes interpretive execution reads.
pub(super) const INTERPRETED_EPISODES: usize = 100;

/// How many whole episodes a published view reads: the first, in recording
/// order, of those the dataset's Selects keep, as the graph shows them kept.
/// Nothing caches a model's answers, so every read of a view asks its models
/// again, and this bounds what one read costs.
pub(super) const PUBLISHED_EPISODES: usize = 20;

/// Two moments this close are one stretch of recording. A moment ends where
/// the next frame starts, so touching moments share their boundary exactly;
/// the published view joins them on that equality.
const TOUCHING_SECONDS: f64 = 1e-6;

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

/// What one node hands on.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Population {
    /// Whole episodes, in sample order.
    Episodes(Vec<EpisodeId>),
    /// Subtasks, episode by episode in sample order, each episode's in time
    /// order, with what the node that read them judged.
    Subtasks(Vec<SubtaskRow>),
    /// Scored moments of one Score progress node, episode by episode in sample
    /// order,
    /// each episode's in time order.
    Moments(Vec<Moment>),
}

impl Population {
    /// The episodes this population reaches, each at its first mention.
    pub(super) fn episodes(&self) -> Vec<EpisodeId> {
        let mentioned: Vec<&EpisodeId> = match self {
            Self::Episodes(episodes) => episodes.iter().collect(),
            Self::Subtasks(rows) => rows.iter().map(SubtaskRow::episode).collect(),
            Self::Moments(rows) => rows.iter().map(Moment::episode).collect(),
        };
        let mut episodes = Vec::<EpisodeId>::new();
        for episode in mentioned {
            if !episodes.contains(episode) {
                episodes.push(episode.clone());
            }
        }
        episodes
    }
    /// The subtask rows this population holds; none unless it holds subtasks.
    pub(super) fn subtasks(&self) -> &[SubtaskRow] {
        match self {
            Self::Subtasks(rows) => rows,
            Self::Episodes(_) | Self::Moments(_) => &[],
        }
    }
    /// How many things it holds, in its own unit.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Episodes(episodes) => episodes.len(),
            Self::Subtasks(rows) => rows.len(),
            Self::Moments(rows) => rows.len(),
        }
    }
    /// One slice per thing it holds, as a published relation holds them:
    /// whole episodes, each subtask, each moment.
    pub(super) fn items(&self, sample: &EpisodeSample) -> Vec<Slice> {
        match self {
            Self::Episodes(episodes) => episodes
                .iter()
                .map(|episode| Slice::of_episode(episode, sample))
                .collect(),
            Self::Subtasks(rows) => rows.iter().map(Slice::of_subtask).collect(),
            Self::Moments(rows) => rows.iter().map(Slice::of_moment).collect(),
        }
    }
    /// The population as training is given it: each episode's kept parts in
    /// time order, with moments that touch joined into one stretch.
    pub(super) fn slices(&self, sample: &EpisodeSample) -> Vec<Slice> {
        match self {
            Self::Episodes(episodes) => episodes
                .iter()
                .map(|episode| Slice::of_episode(episode, sample))
                .filter(|slice| slice.seconds() > 0.0)
                .collect(),
            Self::Subtasks(rows) => rows.iter().map(Slice::of_subtask).collect(),
            Self::Moments(rows) => {
                let mut slices = Vec::<Slice>::new();
                for slice in rows.iter().map(Slice::of_moment) {
                    match slices.last_mut() {
                        Some(last) if last.continues_into(&slice) => last.extend_to(&slice),
                        _ => slices.push(slice),
                    }
                }
                slices
            }
        }
    }
}

/// One part of one episode, in seconds from its start, `start` inclusive and
/// `end` exclusive: the unit a curation finally selects. Training reads the
/// dataset's messages inside it; a subtask's slice also carries the words the
/// annotation described it with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Slice {
    episode: EpisodeId,
    start: f64,
    end: f64,
    label: Option<String>,
}

impl Slice {
    fn new(episode: EpisodeId, start: f64, end: f64, label: Option<String>) -> Self {
        Self {
            episode,
            start,
            end,
            label,
        }
    }
    /// A whole episode, as long as the sample says it is.
    pub(super) fn of_episode(episode: &EpisodeId, sample: &EpisodeSample) -> Self {
        let end = sample.duration(episode).unwrap_or_default().max(0.0);
        Self::new(episode.clone(), 0.0, end, None)
    }
    /// One scored moment's stretch of recording.
    pub(super) fn of_moment(row: &Moment) -> Self {
        Self::new(
            row.episode().clone(),
            row.start_time(),
            row.end_time(),
            None,
        )
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
    pub(super) fn seconds(&self) -> f64 {
        self.end - self.start
    }
    pub(super) fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    /// Whether `next` starts where this slice ends, in the same episode.
    pub(super) fn continues_into(&self, next: &Self) -> bool {
        self.episode == next.episode && next.start - self.end <= TOUCHING_SECONDS
    }
    /// Run this slice on to where `next` ends.
    pub(super) fn extend_to(&mut self, next: &Self) {
        self.end = self.end.max(next.end);
    }
}

/// Reads what each node of one graph hands on over one sample, and what the
/// preview has read over it.
pub(super) struct PopulationReader<'a> {
    graph: &'a CurationGraph,
    sample: &'a EpisodeSample,
    reads: &'a PreviewReads,
}

impl<'a> PopulationReader<'a> {
    pub(super) fn new(
        graph: &'a CurationGraph,
        sample: &'a EpisodeSample,
        reads: &'a PreviewReads,
    ) -> Self {
        Self {
            graph,
            sample,
            reads,
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
    /// What `node` hands on: what it judged, kept by its rule when it has
    /// one. A node the graph does not hold hands on nothing.
    pub(super) fn of(&self, node: &str) -> Population {
        let judged = self.judged(node);
        match self.graph.node(node).and_then(CurationNode::keeps) {
            Some(rule) => self.kept(judged, rule),
            None => judged,
        }
    }
    /// What `node`'s rule judges: what the node measured, or for a Select
    /// what reaches it.
    pub(super) fn judged(&self, node: &str) -> Population {
        let Some(measured) = self.graph.node(node) else {
            return Population::Episodes(Vec::new());
        };
        match measured.kind() {
            CurationNodeKind::Dataset => Population::Episodes(
                self.sample
                    .episodes()
                    .iter()
                    .map(|episode| episode.id().clone())
                    .collect(),
            ),
            // A measurement of subtasks hands on what its own preview read of
            // the episodes that reach it: Break down's cut to the intervals it
            // received, a search's or a grouping's with their judgment.
            CurationNodeKind::BreakDownSubtasks(_)
            | CurationNodeKind::FindSimilarSubtasks(_)
            | CurationNodeKind::GroupSimilarSubtasks(_) => Population::Subtasks(
                self.input(node)
                    .episodes()
                    .iter()
                    .flat_map(|episode| self.reads.subtasks(measured, episode))
                    .cloned()
                    .collect(),
            ),
            // The preview already cut each episode's moments to what reached
            // the node; an episode it has not read yet holds none.
            CurationNodeKind::ScoreProgress(_) => Population::Moments(
                self.input(node)
                    .episodes()
                    .iter()
                    .flat_map(|episode| self.reads.moments(measured, episode))
                    .cloned()
                    .collect(),
            ),
            CurationNodeKind::Select => self.input(node),
        }
    }
    /// What reaches `node`: what its parent hands on.
    pub(super) fn input(&self, node: &str) -> Population {
        match self.graph.parent_of(node) {
            Some(parent) => self.of(parent),
            None => Population::Episodes(Vec::new()),
        }
    }
    /// The part of `judged` `selection` keeps.
    fn kept(&self, judged: Population, selection: &Selection) -> Population {
        match judged {
            Population::Episodes(episodes) => Population::Episodes(
                episodes
                    .into_iter()
                    .filter(|id| {
                        self.sample
                            .episode(id)
                            .is_some_and(|episode| selection.keeps_episode(episode))
                    })
                    .collect(),
            ),
            // Each subtask carries what the measure above judged it to be.
            Population::Subtasks(rows) => Population::Subtasks(
                rows.into_iter()
                    .filter(|row| selection.keeps_subtask(row.similarity(), row.category()))
                    .collect(),
            ),
            Population::Moments(rows) => Population::Moments(
                rows.into_iter()
                    .filter(|row| selection.keeps_moment(row.progress_rate()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::annotate::{CategoryPlacement, JudgmentValue};
    use super::super::document::{
        Comparison, DATASET_NODE_ID, EpisodeCondition, EpisodeFact, ProgressScoring,
        SubtaskAnnotation, SubtaskCategory, SubtaskGrouping, SubtaskSearch,
    };
    use super::super::preview::{PreviewRows, ReadInput};
    use super::*;

    /// A preview that has read nothing.
    fn unread() -> PreviewReads {
        PreviewReads::default()
    }

    fn node(graph: &CurationGraph, id: &str) -> CurationNode {
        graph.node(id).expect("the node is in the graph").clone()
    }

    /// Record that `node` read one episode's moments, one per `(start, rate)`,
    /// each a second long.
    fn read_moments(
        reads: &mut PreviewReads,
        node: &CurationNode,
        episode: &EpisodeId,
        moments: &[(f64, Option<f64>)],
    ) {
        reads.record(
            node,
            (episode, ReadInput::untracked()),
            Ok(PreviewRows::Moments(
                moments
                    .iter()
                    .map(|(start, rate)| {
                        Moment::new(episode.clone(), (*start, start + 1.0), *rate).unwrap()
                    })
                    .collect(),
            )),
        );
    }

    /// Record that `node` read these subtasks of their episode.
    fn read_subtasks(reads: &mut PreviewReads, node: &CurationNode, rows: &[SubtaskRow]) {
        reads.record(
            node,
            (rows[0].episode(), ReadInput::untracked()),
            Ok(PreviewRows::Subtasks(rows.to_vec())),
        );
    }

    fn subtask(episode: u32, (start, end): (f64, f64), description: &str) -> SubtaskRow {
        SubtaskRow::new(EpisodeId::fixture(episode), (start, end), description).unwrap()
    }

    fn sample() -> EpisodeSample {
        EpisodeSample::fixture(vec![
            Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000),
            Episode::fixture(&EpisodeId::fixture(2), 10_000_000_000),
            Episode::fixture(&EpisodeId::fixture(3), 25_000_000_000),
        ])
    }

    fn select(graph: &mut CurationGraph, parent: &str, selection: Selection) -> String {
        graph
            .add_keeping(parent, CurationNodeKind::Select, selection)
            .expect("the grammar allows this Select")
    }

    fn longer_than_20_seconds() -> Selection {
        Selection::episodes_where(EpisodeFact::DurationSeconds, Comparison::AtLeast, 20.0)
    }

    #[test]
    fn a_select_of_episodes_keeps_those_meeting_every_condition_whole() {
        let mut graph = CurationGraph::new(None);
        let between = select(
            &mut graph,
            DATASET_NODE_ID,
            Selection::Episodes {
                conditions: vec![
                    EpisodeCondition::new(EpisodeFact::DurationSeconds, Comparison::AtLeast, 20.0),
                    EpisodeCondition::new(EpisodeFact::DurationSeconds, Comparison::AtMost, 28.0),
                ],
            },
        );
        let sample = sample();
        let reads = unread();
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert_eq!(reader.of(DATASET_NODE_ID).len(), 3);
        assert_eq!(reader.judged(&between).len(), 3);
        let kept = reader.of(&between);
        assert_eq!(kept.episodes(), [EpisodeId::fixture(3)]);
        let slices = kept.slices(&sample);
        assert_eq!((slices[0].start(), slices[0].end()), (0.0, 25.0));
        assert_eq!(graph.outputs().len(), 1);
        assert_eq!(graph.outputs()[0].id(), between);
        // Its conditions are all in the one node, so no Select follows it.
        assert!(
            !graph
                .child_kinds(&between)
                .iter()
                .any(|kind| matches!(kind, CurationNodeKind::Select))
        );
    }

    #[test]
    fn a_breakdown_hands_on_what_it_read_of_the_episodes_that_reach_it() {
        let mut graph = CurationGraph::new(None);
        let long = select(&mut graph, DATASET_NODE_ID, longer_than_20_seconds());
        let annotate = graph
            .add_child(
                &long,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let mut reads = unread();
        for ordinal in [1, 2] {
            read_subtasks(
                &mut reads,
                &node(&graph, &annotate),
                &[subtask(ordinal, (0.0, 4.0), "grasp")],
            );
        }
        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample, &reads);
        // Episode 2 was read before the Select above tightened; it no longer
        // reaches the node, so what it read there is not handed on.
        assert_eq!(reader.of(&annotate).episodes(), [EpisodeId::fixture(1)]);
        assert_eq!(
            reader.of(&annotate).slices(&sample)[0].label(),
            Some("grasp")
        );
        // Asked with another camera, the node has read nothing yet.
        if let CurationNodeKind::BreakDownSubtasks(annotation) =
            graph.node_mut(&annotate).unwrap().kind_mut()
        {
            annotation.set_camera(Some("/top-camera".into()));
        }
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert!(reader.of(&annotate).subtasks().is_empty());
    }

    #[test]
    fn a_grouping_keeps_its_chosen_categories_and_a_search_the_close_subtasks() {
        let mut graph = CurationGraph::new(None);
        let annotate = graph
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let grouping = SubtaskGrouping::of(vec![
            SubtaskCategory::new("Grasp", "Take hold").unwrap(),
            SubtaskCategory::new("Fold", "Bring over").unwrap(),
        ]);
        let group = graph
            .add_keeping(
                &annotate,
                CurationNodeKind::GroupSimilarSubtasks(grouping),
                Selection::Categories {
                    names: vec!["Fold".into()],
                },
            )
            .unwrap();
        // A search inside the chosen category reads only its rows, and keeps
        // those at least as close as its own cutoff.
        let mut query = SubtaskSearch::default();
        query.set_query("fold the towel");
        let search = graph
            .add_keeping(
                &group,
                CurationNodeKind::FindSimilarSubtasks(query),
                Selection::Similar { minimum: 0.5 },
            )
            .unwrap();
        assert_eq!(graph.subtask_source(&search), Some(annotate.as_str()));

        let placed = |row: SubtaskRow, category: Option<&str>| {
            row.judged(
                SubtaskRow::CATEGORY,
                JudgmentValue::Category(
                    category.map(|category| CategoryPlacement::new(category, 0.9).unwrap()),
                ),
            )
        };
        let similar = |row: SubtaskRow, similarity: Option<f64>| {
            row.judged(
                SubtaskRow::SIMILARITY,
                JudgmentValue::Similarity(similarity),
            )
        };
        let (grasp, fold, smooth) = (
            subtask(1, (0.0, 5.0), "grasp the corner"),
            subtask(1, (5.0, 9.0), "fold in half"),
            subtask(1, (9.0, 12.0), "smooth the cloth"),
        );
        let mut reads = unread();
        read_subtasks(
            &mut reads,
            &node(&graph, &annotate),
            &[grasp.clone(), fold.clone(), smooth.clone()],
        );
        read_subtasks(
            &mut reads,
            &node(&graph, &group),
            &[
                placed(grasp, Some("Grasp")),
                placed(fold.clone(), Some("Fold")),
                // A row the model could not place joins no category.
                placed(smooth, None),
            ],
        );
        read_subtasks(
            &mut reads,
            &node(&graph, &search),
            &[
                similar(fold.clone(), Some(0.8)),
                similar(fold.clone(), Some(0.3)),
            ],
        );

        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert_eq!(reader.judged(&group).len(), 3);
        assert_eq!(
            reader
                .of(&group)
                .subtasks()
                .iter()
                .map(SubtaskRow::description)
                .collect::<Vec<_>>(),
            ["fold in half"]
        );
        assert_eq!(reader.judged(&search).len(), 2);
        assert_eq!(
            reader.of(&search).subtasks(),
            [similar(fold.clone(), Some(0.8))]
        );

        // Sliding the cutoff keeps more of what was read, and reads nothing.
        graph
            .set_rule(&search, Selection::Similar { minimum: 0.2 })
            .unwrap();
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert_eq!(reader.of(&search).len(), 2);

        // A new question makes what the search read an answer to another one.
        if let Some(CurationNodeKind::FindSimilarSubtasks(config)) =
            graph.node_mut(&search).map(CurationNode::kind_mut)
        {
            config.set_query("smooth the cloth");
        }
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert!(reader.of(&search).subtasks().is_empty());
    }

    #[test]
    fn score_progress_keeps_moving_and_unread_moments_and_joins_them() {
        let mut graph = CurationGraph::new(None);
        let score = graph
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::ScoreProgress(ProgressScoring::default()),
            )
            .unwrap();
        let episode = EpisodeId::fixture(1);
        let mut reads = unread();
        read_moments(
            &mut reads,
            &node(&graph, &score),
            &episode,
            &[
                (0.0, Some(0.4)),
                (1.0, None),
                (2.0, Some(-0.1)),
                (3.0, Some(0.2)),
            ],
        );
        // How fast is fast enough is the node's own; breaking down what it
        // keeps is what follows.
        assert_eq!(
            graph.node(&score).and_then(CurationNode::keeps),
            Some(&Selection::Progressing { minimum: 0.0 })
        );
        assert_eq!(
            graph
                .child_kinds(&score)
                .iter()
                .map(CurationNodeKind::label)
                .collect::<Vec<_>>(),
            ["Break down subtasks"]
        );
        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample, &reads);
        assert_eq!(reader.judged(&score).len(), 4);
        let slices = reader.of(&score).slices(&sample);
        assert_eq!(
            slices
                .iter()
                .map(|slice| (slice.start(), slice.end()))
                .collect::<Vec<_>>(),
            [(0.0, 2.0), (3.0, 4.0)]
        );
    }

    #[test]
    fn moments_reach_only_the_episodes_that_reach_the_score_node() {
        let mut graph = CurationGraph::new(None);
        let long = select(&mut graph, DATASET_NODE_ID, longer_than_20_seconds());
        let score = graph
            .add_child(
                &long,
                CurationNodeKind::ScoreProgress(ProgressScoring::default()),
            )
            .unwrap();
        // Episode 2 was read before the Select above tightened; it no longer
        // reaches the Score progress node, so its moments are not handed on.
        let score_node = node(&graph, &score);
        let mut reads = unread();
        read_moments(
            &mut reads,
            &score_node,
            &EpisodeId::fixture(1),
            &[(0.0, Some(0.5))],
        );
        read_moments(
            &mut reads,
            &score_node,
            &EpisodeId::fixture(2),
            &[(0.0, None)],
        );
        let sample = sample();
        let moments = PopulationReader::new(&graph, &sample, &reads).of(&score);
        assert_eq!(moments.episodes(), [EpisodeId::fixture(1)]);
        assert_eq!(moments.len(), 1);
    }
}
