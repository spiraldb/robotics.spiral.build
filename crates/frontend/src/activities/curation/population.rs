//! What flows along an edge: which parts of which episodes a node hands on.
//!
//! A curation is shaped against an interpretive sample — the first hundred
//! episodes its dataset lists — so every node answers while the graph is
//! still being built. The dataset hands on those episodes whole; a
//! measurement hands on what it measured over what it received; a Select
//! hands on the part its rule keeps. Any population reads out as slices, one
//! part of one episode each, which is what training is given.
use super::document::{
    CurationGraph, CurationNode, CurationNodeKind, MomentRow, Selection, SubtaskRow,
};
use crate::activities::episode::Episode;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

/// How many of a dataset's episodes interpretive execution reads.
pub(super) const INTERPRETED_EPISODES: usize = 100;

/// Two moments this close are one stretch of recording.
const TOUCHING_SECONDS: f64 = 1e-6;

/// The episodes a curation is shaped against: the first the dataset lists,
/// in recording order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeSample {
    episodes: Vec<Episode>,
    /// Whether the dataset lists more episodes than the sample holds.
    partial: bool,
}

impl EpisodeSample {
    pub(super) async fn load(dataset: &DatasetSelection) -> Result<Self, String> {
        let page = Episode::page(dataset, 0, INTERPRETED_EPISODES).await?;
        Ok(Self {
            partial: page.has_more(),
            episodes: page.into_episodes(),
        })
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
    pub(super) fn is_partial(&self) -> bool {
        self.partial
    }
    #[cfg(test)]
    pub(super) fn fixture(episodes: Vec<Episode>) -> Self {
        Self {
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
    /// Subtask rows of one annotation, in the order it stored them.
    Subtasks(Vec<SubtaskRow>),
    /// Scored moments of one detection, in the order it stored them.
    Moments(Vec<MomentRow>),
}

impl Population {
    /// The episodes this population reaches, each at its first mention.
    pub(super) fn episodes(&self) -> Vec<EpisodeId> {
        let mentioned: Vec<&EpisodeId> = match self {
            Self::Episodes(episodes) => episodes.iter().collect(),
            Self::Subtasks(rows) => rows.iter().map(SubtaskRow::episode).collect(),
            Self::Moments(rows) => rows.iter().map(MomentRow::episode).collect(),
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
    /// The population as training is given it: each episode's kept parts in
    /// time order, with moments that touch joined into one stretch.
    pub(super) fn slices(&self, sample: &EpisodeSample) -> Vec<Slice> {
        match self {
            Self::Episodes(episodes) => episodes
                .iter()
                .filter_map(|episode| {
                    let end = sample.duration(episode).filter(|seconds| *seconds > 0.0)?;
                    Some(Slice::new(episode.clone(), 0.0, end, None))
                })
                .collect(),
            Self::Subtasks(rows) => rows.iter().map(Slice::of_subtask).collect(),
            Self::Moments(rows) => {
                let mut slices = Vec::<Slice>::new();
                for row in rows {
                    match slices.last_mut() {
                        Some(last)
                            if last.episode == *row.episode()
                                && row.start_time() - last.end <= TOUCHING_SECONDS =>
                        {
                            last.end = last.end.max(row.end_time());
                        }
                        _ => slices.push(Slice::new(
                            row.episode().clone(),
                            row.start_time(),
                            row.end_time(),
                            None,
                        )),
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
}

/// Reads what each node of one graph hands on over one sample.
pub(super) struct PopulationReader<'a> {
    graph: &'a CurationGraph,
    sample: &'a EpisodeSample,
}

impl<'a> PopulationReader<'a> {
    pub(super) fn new(graph: &'a CurationGraph, sample: &'a EpisodeSample) -> Self {
        Self { graph, sample }
    }
    /// What `node` hands on. A node the graph does not hold hands on nothing.
    pub(super) fn of(&self, node: &str) -> Population {
        let Some(kind) = self.graph.node(node).map(CurationNode::kind) else {
            return Population::Episodes(Vec::new());
        };
        match kind {
            CurationNodeKind::Dataset => Population::Episodes(
                self.sample
                    .episodes()
                    .iter()
                    .map(|episode| episode.id().clone())
                    .collect(),
            ),
            CurationNodeKind::BreakDownSubtasks(_) => {
                let input = self.input(node).episodes();
                Population::Subtasks(
                    self.graph
                        .subtasks_for(node)
                        .filter(|row| input.contains(row.episode()))
                        .cloned()
                        .collect(),
                )
            }
            // A search scores its input and a grouping places it; both hand
            // all of it on, for a Select to keep part of.
            CurationNodeKind::FindSimilarSubtasks(_) | CurationNodeKind::GroupSimilarSubtasks => {
                self.input(node)
            }
            CurationNodeKind::IdentifyUnproductiveMoments(_) => {
                let input = self.input(node).episodes();
                Population::Moments(
                    self.graph
                        .moments_for(node)
                        .filter(|row| input.contains(row.episode()))
                        .cloned()
                        .collect(),
                )
            }
            CurationNodeKind::Select(selection) => self.select(node, selection),
        }
    }
    /// What reaches `node`: what its parent hands on.
    pub(super) fn input(&self, node: &str) -> Population {
        match self.graph.parent_of(node) {
            Some(parent) => self.of(parent),
            None => Population::Episodes(Vec::new()),
        }
    }
    fn select(&self, node: &str, selection: &Selection) -> Population {
        match self.input(node) {
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
            Population::Subtasks(rows) => {
                let measure = self.graph.selection_measure(node);
                let ranking = measure.and_then(|measure| self.current_ranking(measure));
                let grouping = measure.and_then(|measure| self.graph.grouping_for(measure.id()));
                Population::Subtasks(
                    rows.into_iter()
                        .filter(|row| {
                            selection.keeps_subtask(
                                ranking.and_then(|ranking| ranking.probability(row)),
                                grouping.and_then(|grouping| grouping.category_of(row)),
                            )
                        })
                        .collect(),
                )
            }
            Population::Moments(rows) => Population::Moments(
                rows.into_iter()
                    .filter(|row| selection.keeps_moment(row.velocity()))
                    .collect(),
            ),
        }
    }
    /// A search's stored ranking, while it still answers the search's query.
    /// A ranking of an earlier question says nothing about this one.
    fn current_ranking(
        &self,
        measure: &CurationNode,
    ) -> Option<&'a super::document::SubtaskRanking> {
        let CurationNodeKind::FindSimilarSubtasks(search) = measure.kind() else {
            return None;
        };
        self.graph
            .ranking_for(measure.id())
            .filter(|ranking| ranking.query().trim() == search.query().trim())
    }
}

#[cfg(test)]
mod tests {
    use super::super::document::{
        Comparison, DATASET_NODE_ID, EpisodeFact, MomentDetection, SubtaskAnnotation,
        SubtaskCategory, SubtaskGrouping, SubtaskAssignment, SubtaskRanking, SubtaskSearch,
    };
    use super::*;

    fn sample() -> EpisodeSample {
        EpisodeSample::fixture(vec![
            Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000),
            Episode::fixture(&EpisodeId::fixture(2), 10_000_000_000),
            Episode::fixture(&EpisodeId::fixture(3), 25_000_000_000),
        ])
    }

    fn select(graph: &mut CurationGraph, parent: &str, selection: Selection) -> String {
        graph
            .add_child(parent, CurationNodeKind::Select(selection))
            .expect("the grammar allows this Select")
    }

    #[test]
    fn chained_episode_selects_narrow_and_hand_on_whole_episodes() {
        let mut graph = CurationGraph::new(None);
        let long = select(
            &mut graph,
            DATASET_NODE_ID,
            Selection::Episodes {
                fact: EpisodeFact::DurationSeconds,
                comparison: Comparison::AtLeast,
                value: 20.0,
            },
        );
        let shorter = select(
            &mut graph,
            &long,
            Selection::Episodes {
                fact: EpisodeFact::DurationSeconds,
                comparison: Comparison::AtMost,
                value: 28.0,
            },
        );
        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample);
        assert_eq!(reader.of(DATASET_NODE_ID).len(), 3);
        assert_eq!(
            reader.of(&long).episodes(),
            [EpisodeId::fixture(1), EpisodeId::fixture(3)]
        );
        let kept = reader.of(&shorter);
        assert_eq!(kept.episodes(), [EpisodeId::fixture(3)]);
        let slices = kept.slices(&sample);
        assert_eq!((slices[0].start(), slices[0].end()), (0.0, 25.0));
        assert_eq!(graph.outputs().len(), 1);
        assert_eq!(graph.outputs()[0].id(), shorter);
    }

    #[test]
    fn an_annotation_reads_only_the_episodes_that_reach_it() {
        let mut graph = CurationGraph::new(None);
        let long = select(
            &mut graph,
            DATASET_NODE_ID,
            Selection::Episodes {
                fact: EpisodeFact::DurationSeconds,
                comparison: Comparison::AtLeast,
                value: 20.0,
            },
        );
        let annotate = graph
            .add_child(
                &long,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        for ordinal in [1, 2] {
            let episode = EpisodeId::fixture(ordinal);
            graph
                .replace_episode_subtasks(
                    &annotate,
                    &episode,
                    vec![SubtaskRow::new(&annotate, episode.clone(), 0.0, 4.0, "grasp").unwrap()],
                )
                .unwrap();
        }
        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample);
        // Episode 2 was annotated before the Select above tightened; its rows
        // are kept in the document but no longer reach anything.
        assert_eq!(
            reader.of(&annotate).episodes(),
            [EpisodeId::fixture(1)]
        );
        assert_eq!(
            reader.of(&annotate).slices(&sample)[0].label(),
            Some("grasp")
        );
    }

    #[test]
    fn subtask_selects_keep_by_the_search_or_grouping_above_them() {
        let mut graph = CurationGraph::new(None);
        let annotate = graph
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let episode = EpisodeId::fixture(1);
        let rows = vec![
            SubtaskRow::new(&annotate, episode.clone(), 0.0, 5.0, "grasp the corner").unwrap(),
            SubtaskRow::new(&annotate, episode.clone(), 5.0, 9.0, "fold in half").unwrap(),
        ];
        graph
            .replace_episode_subtasks(&annotate, &episode, rows.clone())
            .unwrap();
        let group = graph
            .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks)
            .unwrap();
        graph
            .set_grouping(
                SubtaskGrouping::new(
                    &group,
                    &annotate,
                    vec![
                        SubtaskCategory::new("Grasp", "Take hold").unwrap(),
                        SubtaskCategory::new("Fold", "Bring over").unwrap(),
                    ],
                    vec![
                        SubtaskAssignment::new(episode.clone(), 0.0, "Grasp", 0.9, 0.8).unwrap(),
                        SubtaskAssignment::new(episode.clone(), 5.0, "Fold", 0.9, 0.8).unwrap(),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
        let folds = select(
            &mut graph,
            &group,
            Selection::Categories {
                names: vec!["Fold".into()],
            },
        );
        // A search inside the chosen category reads only its rows.
        let search = graph
            .add_child(
                &folds,
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        assert_eq!(graph.subtask_source(&search), Some(annotate.as_str()));
        if let Some(CurationNodeKind::FindSimilarSubtasks(config)) =
            graph.node_mut(&search).map(CurationNode::kind_mut)
        {
            config.set_query("fold the towel");
        }
        graph
            .set_ranking(
                SubtaskRanking::new(&search, "fold the towel", [(rows[1].key(), 0.8)]).unwrap(),
            )
            .unwrap();
        let close = select(&mut graph, &search, Selection::Similar { minimum: 0.5 });

        let sample = sample();
        let reader = PopulationReader::new(&graph, &sample);
        assert_eq!(reader.of(&folds).subtasks(), &rows[1..]);
        assert_eq!(reader.of(&search).subtasks(), &rows[1..]);
        assert_eq!(reader.of(&close).subtasks(), &rows[1..]);

        // A new question makes the stored ranking an answer to another one.
        if let Some(CurationNodeKind::FindSimilarSubtasks(config)) =
            graph.node_mut(&search).map(CurationNode::kind_mut)
        {
            config.set_query("smooth the cloth");
        }
        let reader = PopulationReader::new(&graph, &sample);
        assert!(reader.of(&close).subtasks().is_empty());
    }

    #[test]
    fn a_progress_select_keeps_moving_and_unread_moments_and_joins_them() {
        let mut graph = CurationGraph::new(None);
        let detect = graph
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
            )
            .unwrap();
        let episode = EpisodeId::fixture(1);
        let moment = |start: f64, velocity: Option<f64>| {
            MomentRow::new(&detect, episode.clone(), (start, start + 1.0), velocity).unwrap()
        };
        graph
            .replace_episode_moments(
                &detect,
                &episode,
                vec![
                    moment(0.0, Some(0.4)),
                    moment(1.0, None),
                    moment(2.0, Some(-0.1)),
                    moment(3.0, Some(0.2)),
                ],
            )
            .unwrap();
        let moving = select(&mut graph, &detect, Selection::Progressing { minimum: 0.0 });
        assert!(
            graph.child_kinds(&moving).is_empty(),
            "a second Select of the same progress would only repeat this one"
        );
        let sample = sample();
        let slices = PopulationReader::new(&graph, &sample)
            .of(&moving)
            .slices(&sample);
        assert_eq!(
            slices
                .iter()
                .map(|slice| (slice.start(), slice.end()))
                .collect::<Vec<_>>(),
            [(0.0, 2.0), (3.0, 4.0)]
        );
    }
}
