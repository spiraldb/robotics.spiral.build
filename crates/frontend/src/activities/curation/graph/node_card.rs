//! What one node of the curation graph says, and what its + offers. Everything
//! here reads the payload and the sample it is interpreted over, so the
//! canvas keeps no second copy of the graph.
use spiral_robotics_common::datasets::DatasetSelection;

use super::super::document::{
    CurationGraph, CurationNode, CurationNodeKind, DATASET_NODE_ID, PopulationKind, PublishedOutput,
};
use super::super::population::{EpisodeSample, Population, PopulationReader, Slice};
use super::super::preview::PreviewProgress;
use super::super::review::Counted;
use super::super::state::CurationController;
use crate::format::ClockPresentation as _;

/// What a node does to the data; drives its glyph and identity tint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::activities::curation) enum NodeFace {
    Dataset,
    Annotation,
    Search,
    Grouping,
    Detection,
    Select,
}

impl NodeFace {
    pub(in crate::activities::curation) fn of(kind: &CurationNodeKind) -> Self {
        match kind {
            CurationNodeKind::Dataset => Self::Dataset,
            CurationNodeKind::BreakDownSubtasks(_) => Self::Annotation,
            CurationNodeKind::FindSimilarSubtasks(_) => Self::Search,
            CurationNodeKind::GroupSimilarSubtasks(_) => Self::Grouping,
            CurationNodeKind::IdentifyUnproductiveMoments(_) => Self::Detection,
            CurationNodeKind::Select(_) => Self::Select,
        }
    }
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Dataset => "workflow-stage-source",
            Self::Annotation | Self::Detection => "workflow-stage-model",
            Self::Search => "workflow-stage-media",
            Self::Grouping => "workflow-stage-join",
            Self::Select => "workflow-stage-shape",
        }
    }
}

/// What the + on a node offers, and what choosing one does. A node whose
/// output supports nothing has no menu and draws no +.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChildMenu {
    parent: String,
    kinds: Vec<CurationNodeKind>,
}

impl ChildMenu {
    /// What the grammar allows after `node`. A node that cannot run on this
    /// dataset is still offered; its workspace says why.
    fn of(payload: &CurationGraph, node: &CurationNode) -> Option<Self> {
        let kinds = payload.child_kinds(node.id());
        (!kinds.is_empty()).then(|| Self {
            parent: node.id().to_owned(),
            kinds,
        })
    }
    pub(super) fn labels(&self) -> Vec<&'static str> {
        self.kinds.iter().map(CurationNodeKind::label).collect()
    }
    /// Add the chosen child to the graph and select it, so the workspace
    /// opens on the node the user just created.
    pub(super) fn choose(
        &self,
        controller: CurationController,
        index: usize,
    ) -> Result<String, String> {
        let kind = self
            .kinds
            .get(index)
            .cloned()
            .ok_or("That action is no longer offered.")?;
        let parent = self.parent.clone();
        let id = controller.edit(move |payload| payload.add_child(&parent, kind))?;
        controller.select_node(Some(id.clone()));
        Ok(id)
    }
}

/// The one line under a card's title: what the node holds, or — while it
/// runs — what it is doing, which carries the live count in its place.
#[derive(Clone, Debug, PartialEq)]
enum CardLine {
    Holds(String),
    Running(String),
}

/// Whether an output is published, and as what.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum OutputFlag {
    Unpublished,
    Published(PublishedOutput),
}

/// One node as the canvas draws it: what it is, one line of what it holds or
/// is doing, whether it is one of the curation's outputs, and what may follow
/// it.
///
/// The run line is part of the card rather than read beside it: the canvas
/// redraws a node when its card changes, so a line kept anywhere else went
/// on saying "Annotating 6 of 6 episodes" after the run had finished.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NodeCard {
    id: String,
    face: NodeFace,
    label: &'static str,
    line: CardLine,
    output: Option<OutputFlag>,
    menu: Option<ChildMenu>,
    order: usize,
}

impl NodeCard {
    /// Every node of the graph, in payload order. Counts wait for the sample.
    pub(super) fn all(payload: &CurationGraph, reader: Option<&PopulationReader<'_>>) -> Vec<Self> {
        let outputs = payload
            .outputs()
            .into_iter()
            .map(|node| node.id().to_owned())
            .collect::<Vec<_>>();
        payload
            .nodes()
            .iter()
            .map(|node| Self {
                id: node.id().to_owned(),
                face: NodeFace::of(node.kind()),
                label: node.kind().label(),
                line: CardLine::Holds(Self::describe(payload, node, reader)),
                output: outputs.iter().any(|output| output == node.id()).then(|| {
                    payload
                        .publication(node.id())
                        .cloned()
                        .map_or(OutputFlag::Unpublished, OutputFlag::Published)
                }),
                menu: ChildMenu::of(payload, node),
                order: 0,
            })
            .collect()
    }
    /// One line saying what this node holds right now. Every card's line is
    /// what the node holds, never a second copy of the instruction its open
    /// workspace is already giving.
    fn describe(
        payload: &CurationGraph,
        node: &CurationNode,
        reader: Option<&PopulationReader<'_>>,
    ) -> String {
        let sample = reader.map(PopulationReader::sample);
        let counts = reader.map(|reader| (reader.input(node.id()), reader.of(node.id())));
        match node.kind() {
            CurationNodeKind::Dataset => {
                // The name the Dataset workspace and Data show, without the
                // storage prefix that only pushed the count to a second line.
                let name = payload.dataset().map_or_else(
                    || "No dataset yet".to_owned(),
                    DatasetSelection::display_name,
                );
                // Every count below this card is over the episodes it reads, so
                // a dataset read only in part says so here, once.
                let partial = sample.is_some_and(EpisodeSample::is_partial);
                match counts {
                    Some((_, episodes)) if payload.dataset().is_some() && partial => {
                        format!("{name} · first {}", Counted::episodes(episodes.len()))
                    }
                    Some((_, episodes)) if payload.dataset().is_some() => {
                        format!("{name} · {}", Counted::episodes(episodes.len()))
                    }
                    _ => name,
                }
            }
            // What the node holds, not what it runs with: the camera and model
            // are the workspace's status line.
            CurationNodeKind::BreakDownSubtasks(_) => match (counts, reader) {
                (Some((input, output)), Some(reader)) => Self::stale(
                    format!(
                        "{} of {} · {}",
                        PreviewProgress::of(reader, node.id()).read().len(),
                        Counted::episodes(input.episodes().len()),
                        Counted::subtasks(output.len()),
                    ),
                    node,
                    reader,
                ),
                _ => "Subtasks of every episode".to_owned(),
            },
            CurationNodeKind::FindSimilarSubtasks(search) => {
                let query = search.query().trim();
                let line = if query.is_empty() {
                    "No query yet".to_owned()
                } else {
                    format!("“{query}”")
                };
                reader.map_or_else(
                    || line.clone(),
                    |reader| Self::stale(line.clone(), node, reader),
                )
            }
            CurationNodeKind::GroupSimilarSubtasks(grouping) => {
                let line = match grouping.categories().len() {
                    0 => "No categories yet".to_owned(),
                    categories => Counted::categories(categories).to_string(),
                };
                reader.map_or_else(
                    || line.clone(),
                    |reader| Self::stale(line.clone(), node, reader),
                )
            }
            // A moment is one frame, so what the detection holds is said as
            // the recording it scored rather than a count of frames.
            CurationNodeKind::IdentifyUnproductiveMoments(_) => match (counts, reader) {
                (Some((input, output)), Some(reader)) => Self::stale(
                    format!(
                        "{} of {} · {} scored",
                        PreviewProgress::of(reader, node.id()).read().len(),
                        Counted::episodes(input.len()),
                        Self::recorded(&output, reader.sample()),
                    ),
                    node,
                    reader,
                ),
                _ => "Pace per frame of recording".to_owned(),
            },
            CurationNodeKind::Select(selection) => match (counts, sample) {
                // A moment is a frame, so what a rule over moments keeps is
                // said as the recording it adds up to: what training gets.
                (Some((input, output)), Some(sample))
                    if payload.population_kind(node.id()) == Some(PopulationKind::Moments) =>
                {
                    format!(
                        "{} · keeps {} of {}",
                        selection.sentence(),
                        Self::recorded(&output, sample),
                        Self::recorded(&input, sample)
                    )
                }
                (Some((input, output)), _) => {
                    let unit = payload.population_kind(node.id());
                    format!(
                        "{} · {} of {}",
                        selection.sentence(),
                        output.len(),
                        Counted::of_kind(unit, input.len())
                    )
                }
                (None, _) => selection.sentence(),
            },
        }
    }
    /// How much recording a population adds up to, on the clock.
    fn recorded(population: &Population, sample: &EpisodeSample) -> String {
        population
            .slices(sample)
            .iter()
            .map(Slice::seconds)
            .sum::<f64>()
            .clock_label()
    }
    /// A measurement's line, saying when what reaches it changed since its
    /// preview ran.
    fn stale(line: String, node: &CurationNode, reader: &PopulationReader<'_>) -> String {
        if PreviewProgress::of(reader, node.id()).is_stale() {
            format!("{line} · out of date")
        } else {
            line
        }
    }
    pub(super) fn with_order(mut self, order: usize) -> Self {
        self.order = order;
        self
    }
    /// This card while its node runs, saying `activity` in place of what it
    /// holds.
    pub(super) fn running(mut self, activity: Option<&String>) -> Self {
        if let Some(activity) = activity {
            self.line = CardLine::Running(activity.clone());
        }
        self
    }
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn face(&self) -> NodeFace {
        self.face
    }
    pub(super) fn label(&self) -> &'static str {
        self.label
    }
    /// The words under the title, whichever they are.
    pub(super) fn summary(&self) -> &str {
        match &self.line {
            CardLine::Holds(words) | CardLine::Running(words) => words,
        }
    }
    pub(super) fn is_running(&self) -> bool {
        matches!(self.line, CardLine::Running(_))
    }
    /// Whether this node is one of the curation's outputs, and published.
    pub(super) fn output(&self) -> Option<&OutputFlag> {
        self.output.as_ref()
    }
    pub(super) fn menu(&self) -> Option<&ChildMenu> {
        self.menu.as_ref()
    }
    /// The dataset node is the graph's root and stays.
    pub(super) fn removable(&self) -> bool {
        self.id != DATASET_NODE_ID
    }
    pub(super) fn order(&self) -> usize {
        self.order
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::annotate::SubtaskRow;
    use super::super::super::document::{
        Comparison, EpisodeFact, Selection, SubtaskAnnotation, SubtaskGrouping, SubtaskSearch,
    };
    use super::super::super::preview::{PreviewReads, PreviewRows, ReadInput};
    use super::*;
    use crate::activities::episode::Episode;
    use spiral_robotics_common::datasets::EpisodeId;

    fn card<'a>(cards: &'a [NodeCard], id: &str) -> &'a NodeCard {
        cards.iter().find(|card| card.id() == id).expect("card")
    }

    #[test]
    fn a_new_graph_asks_for_a_dataset_and_offers_every_measurement() {
        let payload = CurationGraph::new(None);
        let cards = NodeCard::all(&payload, None);
        let dataset = card(&cards, DATASET_NODE_ID);
        assert_eq!(dataset.summary(), "No dataset yet");
        assert_eq!(dataset.face(), NodeFace::Dataset);
        assert!(!dataset.removable());
        assert_eq!(
            dataset
                .menu()
                .expect("the dataset accepts children")
                .labels(),
            [
                "Break down subtasks",
                "Identify unproductive moments",
                "Select"
            ]
        );
    }

    #[test]
    fn each_kind_says_what_it_holds_in_one_line() {
        let mut payload =
            CurationGraph::new(DatasetSelection::new(vec!["cloth_fold_demo".to_owned()]).ok());
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let sample = EpisodeSample::fixture(vec![
            Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000),
            Episode::fixture(&EpisodeId::fixture(2), 10_000_000_000),
        ]);
        let mut reads = PreviewReads::default();
        let input = ReadInput::current(
            &PopulationReader::new(&payload, &sample, &reads),
            &annotate,
            &EpisodeId::fixture(1),
        );
        reads.record(
            payload.node(&annotate).unwrap(),
            (&EpisodeId::fixture(1), input),
            Ok(PreviewRows::Subtasks(vec![
                SubtaskRow::new(EpisodeId::fixture(1), (0.0, 8.0), "grasp").unwrap(),
            ])),
        );
        let search = payload
            .add_child(
                &annotate,
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(
                &annotate,
                CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
            )
            .unwrap();
        let long = payload
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::Select(Selection::Episodes {
                    fact: EpisodeFact::DurationSeconds,
                    comparison: Comparison::AtLeast,
                    value: 20.0,
                }),
            )
            .unwrap();
        let reader = PopulationReader::new(&payload, &sample, &reads);
        let cards = NodeCard::all(&payload, Some(&reader));
        assert_eq!(
            card(&cards, DATASET_NODE_ID).summary(),
            "cloth_fold_demo · 2 episodes"
        );
        assert_eq!(
            card(&cards, &annotate).summary(),
            "1 of 2 episodes · 1 subtask"
        );
        assert_eq!(card(&cards, &search).summary(), "No query yet");
        assert_eq!(card(&cards, &group).summary(), "No categories yet");
        assert_eq!(
            card(&cards, &long).summary(),
            "Duration at least 20 s · 1 of 2 episodes"
        );
        assert_eq!(card(&cards, &long).output(), Some(&OutputFlag::Unpublished));
        assert_eq!(card(&cards, &annotate).output(), None);
        assert_eq!(card(&cards, &long).face(), NodeFace::Select);
        assert_eq!(
            card(&cards, &annotate).menu().expect("menu").labels(),
            ["Find similar subtasks", "Group similar subtasks"]
        );
        assert_eq!(
            card(&cards, &group).menu().expect("menu").labels(),
            ["Select"]
        );
        assert!(card(&cards, &annotate).removable());

        if let CurationNodeKind::BreakDownSubtasks(annotation) =
            payload.node_mut(&annotate).unwrap().kind_mut()
        {
            annotation.set_camera(Some("/camera/top".to_owned()));
        }
        if let CurationNodeKind::FindSimilarSubtasks(config) =
            payload.node_mut(&search).unwrap().kind_mut()
        {
            config.set_query("make the first fold");
        }
        let cards = NodeCard::all(&payload, None);
        assert_eq!(
            card(&cards, &annotate).summary(),
            "Subtasks of every episode"
        );
        assert_eq!(card(&cards, &search).summary(), "“make the first fold”");
        // With another camera the node has read nothing, as the preview says.
        let reader = PopulationReader::new(&payload, &sample, &reads);
        let cards = NodeCard::all(&payload, Some(&reader));
        assert_eq!(
            card(&cards, &annotate).summary(),
            "0 of 2 episodes · 0 subtasks"
        );
    }
}
