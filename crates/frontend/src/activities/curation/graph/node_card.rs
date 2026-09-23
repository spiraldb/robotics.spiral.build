//! What one node of the curation graph says, and what its + offers. Everything
//! here reads the payload and the sample it is interpreted over, so the
//! canvas keeps no second copy of the graph.
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};

use super::super::document::{CurationGraph, CurationNode, CurationNodeKind, DATASET_NODE_ID};
use super::super::population::{EpisodeSample, Population, PopulationReader};
use super::super::review::Counted;
use super::super::state::CurationController;

/// What a node does to the data; drives its glyph and identity tint, in the
/// same vocabulary the Workflow graph speaks.
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
            CurationNodeKind::GroupSimilarSubtasks => Self::Grouping,
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

/// One node as the canvas draws it: what it is, one line of what it holds,
/// whether it is one of the curation's outputs, and what may follow it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NodeCard {
    id: String,
    face: NodeFace,
    label: &'static str,
    summary: String,
    output: bool,
    menu: Option<ChildMenu>,
    order: usize,
}

impl NodeCard {
    /// Every node of the graph, in payload order. Counts wait for the sample.
    pub(super) fn all(payload: &CurationGraph, sample: Option<&EpisodeSample>) -> Vec<Self> {
        let outputs = payload
            .outputs()
            .into_iter()
            .map(|node| node.id().to_owned())
            .collect::<Vec<_>>();
        let reader = sample.map(|sample| PopulationReader::new(payload, sample));
        payload
            .nodes()
            .iter()
            .map(|node| Self {
                id: node.id().to_owned(),
                face: NodeFace::of(node.kind()),
                label: node.kind().label(),
                summary: Self::describe(payload, node, reader.as_ref()),
                output: outputs.iter().any(|output| output == node.id()),
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
        let counts = reader.map(|reader| (reader.input(node.id()), reader.of(node.id())));
        match node.kind() {
            CurationNodeKind::Dataset => {
                let name = payload
                    .dataset()
                    .map_or_else(|| "No dataset yet".to_owned(), DatasetSelection::label);
                match counts {
                    Some((_, episodes)) if payload.dataset().is_some() => {
                        format!("{name} · {}", Counted::episodes(episodes.len()))
                    }
                    _ => name,
                }
            }
            CurationNodeKind::BreakDownSubtasks(annotation) => {
                let measured = match counts {
                    Some((input, output)) => format!(
                        "{} of {} · {}",
                        Self::measured(annotation.completed_episodes(), &input),
                        Counted::episodes(input.episodes().len()),
                        Counted::subtasks(output.len()),
                    ),
                    None => Counted::subtasks(payload.subtasks_for(node.id()).count()).to_string(),
                };
                match annotation.camera() {
                    Some(camera) => format!("{measured} · {camera}"),
                    None => measured,
                }
            }
            CurationNodeKind::FindSimilarSubtasks(search) => {
                let query = search.query().trim();
                if query.is_empty() {
                    "No query yet".to_owned()
                } else {
                    format!("“{query}”")
                }
            }
            CurationNodeKind::GroupSimilarSubtasks => payload.grouping_for(node.id()).map_or_else(
                || "No categories yet".to_owned(),
                |grouping| Counted::categories(grouping.categories().len()).to_string(),
            ),
            CurationNodeKind::IdentifyUnproductiveMoments(detection) => match counts {
                Some((input, output)) => format!(
                    "{} of {} · {}",
                    Self::measured(detection.completed_episodes(), &input),
                    Counted::episodes(input.len()),
                    Counted::moments(output.len()),
                ),
                None => "Progress per second of recording".to_owned(),
            },
            CurationNodeKind::Select(selection) => match counts {
                Some((input, output)) => {
                    let unit = payload.population_kind(node.id());
                    format!(
                        "{} · {} of {}",
                        selection.sentence(),
                        output.len(),
                        Counted::of_kind(unit, input.len())
                    )
                }
                None => selection.sentence(),
            },
        }
    }
    /// How many of the episodes reaching a measurement it has read.
    fn measured(completed: &[EpisodeId], input: &Population) -> usize {
        let input = input.episodes();
        completed
            .iter()
            .filter(|episode| input.contains(episode))
            .count()
    }
    pub(super) fn with_order(mut self, order: usize) -> Self {
        self.order = order;
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
    pub(super) fn summary(&self) -> &str {
        &self.summary
    }
    /// Whether this node is one of the curation's outputs.
    pub(super) fn is_output(&self) -> bool {
        self.output
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
    use super::super::super::document::{
        Comparison, EpisodeFact, Selection, SubtaskAnnotation, SubtaskRow, SubtaskSearch,
    };
    use super::*;
    use crate::activities::episode::Episode;

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
        payload
            .replace_episode_subtasks(
                &annotate,
                &EpisodeId::fixture(1),
                vec![SubtaskRow::new(&annotate, EpisodeId::fixture(1), 0.0, 8.0, "grasp").unwrap()],
            )
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(&annotate, CurationNodeKind::GroupSimilarSubtasks)
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
        let sample = EpisodeSample::fixture(vec![
            Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000),
            Episode::fixture(&EpisodeId::fixture(2), 10_000_000_000),
        ]);

        let cards = NodeCard::all(&payload, Some(&sample));
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
        assert!(card(&cards, &long).is_output());
        assert!(!card(&cards, &annotate).is_output());
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
        assert_eq!(card(&cards, &annotate).summary(), "1 subtask · /camera/top");
        assert_eq!(card(&cards, &search).summary(), "“make the first fold”");
    }
}
