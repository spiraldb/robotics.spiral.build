//! What one node of the curation graph says, and what its + offers. Everything
//! here reads the payload and the sample it is interpreted over, so the
//! canvas keeps no second copy of the graph.
use spiral_robotics_common::datasets::DatasetSelection;

use super::super::document::{
    CurationGraph, CurationNode, CurationNodeKind, DATASET_NODE_ID, PublishedOutput,
};
use super::super::population::{EpisodeSample, PopulationReader};
use super::super::preview::PreviewProgress;
use super::super::review::Counted;
use super::super::state::CurationController;

/// What a node does to the data; drives its glyph and identity tint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::activities::curation) enum NodeFace {
    Dataset,
    Annotation,
    Search,
    Grouping,
    Scoring,
    Tracking,
    Select,
}

impl NodeFace {
    pub(in crate::activities::curation) fn of(kind: &CurationNodeKind) -> Self {
        match kind {
            CurationNodeKind::Dataset => Self::Dataset,
            CurationNodeKind::BreakDownSubtasks(_) => Self::Annotation,
            CurationNodeKind::FindSimilarSubtasks(_) => Self::Search,
            CurationNodeKind::GroupSimilarSubtasks(_) => Self::Grouping,
            CurationNodeKind::ScoreProgress(_) => Self::Scoring,
            CurationNodeKind::TrackObjects(_) => Self::Tracking,
            CurationNodeKind::Select(_) => Self::Select,
        }
    }
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Dataset => "curation-canvas-stage-source",
            Self::Annotation | Self::Scoring | Self::Tracking => "curation-canvas-stage-model",
            Self::Search => "curation-canvas-stage-media",
            Self::Grouping => "curation-canvas-stage-join",
            Self::Select => "curation-canvas-stage-shape",
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

/// One node as the canvas draws it: what it is, one line of what it holds or
/// is doing, the view it is published as, and what may follow it.
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
    published: Option<PublishedOutput>,
    menu: Option<ChildMenu>,
    order: usize,
}

impl NodeCard {
    /// Every node of the graph, in payload order. Counts wait for the sample.
    pub(super) fn all(payload: &CurationGraph, reader: Option<&PopulationReader<'_>>) -> Vec<Self> {
        payload
            .nodes()
            .iter()
            .map(|node| Self {
                id: node.id().to_owned(),
                face: NodeFace::of(node.kind()),
                label: node.kind().label(),
                line: CardLine::Holds(Self::describe(payload, node, reader)),
                published: payload.publication(node.id()).cloned(),
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
        // How far a model node's preview has read what reaches it, or that it
        // has read nothing yet: what is waiting for it, rather than zeros
        // that read as a result.
        let measured = |reader: &PopulationReader<'_>| {
            let reaching = reader.input(node.id()).len();
            let progress = PreviewProgress::of(reader, node.id());
            if progress.is_unmeasured() {
                format!("Not run yet · {}", Counted::episodes(reaching))
            } else {
                format!(
                    "{} of {}",
                    progress.read().len(),
                    Counted::episodes(reaching)
                )
            }
        };
        let judged = |reader: &PopulationReader<'_>| {
            Counted::subtasks(reader.subtasks_reaching(node.id()).len()).to_string()
        };
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
            CurationNodeKind::BreakDownSubtasks(_) => match reader {
                Some(reader) if !PreviewProgress::of(reader, node.id()).is_unmeasured() => {
                    format!("{} · {}", measured(reader), judged(reader))
                }
                Some(reader) => measured(reader),
                None => "Subtasks of every episode".to_owned(),
            },
            // The question, then how many subtasks it has answered for.
            CurationNodeKind::FindSimilarSubtasks(search) => {
                let asked = match search.query().trim() {
                    "" => "No query yet".to_owned(),
                    query => format!("“{query}”"),
                };
                match reader {
                    Some(reader) if !PreviewProgress::of(reader, node.id()).is_unmeasured() => {
                        format!("{asked} · {}", judged(reader))
                    }
                    _ => asked,
                }
            }
            // How many categories, then how many subtasks it grouped.
            CurationNodeKind::GroupSimilarSubtasks(grouping) => {
                let line = match grouping.categories().len() {
                    0 => "No categories yet".to_owned(),
                    categories => Counted::categories(categories).to_string(),
                };
                match reader {
                    Some(reader) if !PreviewProgress::of(reader, node.id()).is_unmeasured() => {
                        format!("{line} · {}", judged(reader))
                    }
                    _ => line,
                }
            }
            CurationNodeKind::ScoreProgress(_) => reader
                .map(measured)
                .unwrap_or_else(|| "Progress rate of every moment".to_owned()),
            // What it tracks, then how far it has got.
            CurationNodeKind::TrackObjects(tracking) => {
                let tracked = match tracking.prompt() {
                    "" => "Nothing named yet".to_owned(),
                    _ => format!("{} at {}/s", tracking.plural(), tracking.rate()),
                };
                match reader {
                    Some(reader) => format!("{tracked} · {}", measured(reader)),
                    None => tracked,
                }
            }
            CurationNodeKind::Select(selection) => {
                let sentence = selection.sentence(payload);
                match counts {
                    Some((input, output)) => format!(
                        "{sentence} · {} of {}",
                        output.len(),
                        Counted::episodes(input.len())
                    ),
                    None => sentence,
                }
            }
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
    /// The view the node is published as, when it is.
    pub(super) fn published(&self) -> Option<&PublishedOutput> {
        self.published.as_ref()
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
        Comparison, EpisodeFact, Selection, SubtaskAnnotation, SubtaskCategory, SubtaskGrouping,
        SubtaskSearch,
    };
    use super::super::super::path::CurationPath;
    use super::super::super::preview::{PreviewReads, PreviewRows};
    use super::*;
    use crate::activities::episode::Episode;
    use spiral_robotics_common::datasets::EpisodeId;

    fn card<'a>(cards: &'a [NodeCard], id: &str) -> &'a NodeCard {
        cards.iter().find(|card| card.id() == id).expect("card")
    }

    #[test]
    fn a_new_graph_asks_for_a_dataset_and_offers_every_node() {
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
                "Score progress",
                "Track objects",
                "Select"
            ]
        );
    }

    #[test]
    fn each_kind_says_what_it_holds_in_one_line() {
        let mut payload = CurationGraph::new(Some(
            crate::activities::episode::EpisodeListing::mcap_fixture("cloth_fold_demo"),
        ));
        let mut annotation = SubtaskAnnotation::default();
        annotation.set_camera(Some("/camera/top".to_owned()));
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                CurationNodeKind::BreakDownSubtasks(annotation),
            )
            .unwrap();
        let sample = EpisodeSample::fixture(vec![
            Episode::fixture(&EpisodeId::fixture(1), 30_000_000_000),
            Episode::fixture(&EpisodeId::fixture(2), 10_000_000_000),
        ]);
        let column = CurationPath::to(&payload, &annotate)
            .unwrap()
            .answer_column()
            .unwrap();
        let mut reads = PreviewReads::default();
        reads.record(
            (&annotate, &column),
            &EpisodeId::fixture(1),
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
                CurationNodeKind::Select(Selection::keeping_everything()),
            )
            .unwrap();
        payload
            .set_selection(
                &long,
                Selection::episodes_where(EpisodeFact::DurationSeconds, Comparison::AtLeast, 20.0),
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
        assert_eq!(card(&cards, &long).published(), None);
        assert_eq!(card(&cards, &long).face(), NodeFace::Select);
        assert!(card(&cards, &annotate).removable());

        if let CurationNodeKind::FindSimilarSubtasks(config) =
            payload.node_mut(&search).unwrap().kind_mut()
        {
            config.set_query("make the first fold");
        }
        payload
            .propose_categories(
                &group,
                vec![
                    SubtaskCategory::new("Grasp", "Take hold.").unwrap(),
                    SubtaskCategory::new("Fold", "Bring over.").unwrap(),
                ],
            )
            .unwrap();
        let cards = NodeCard::all(&payload, None);
        assert_eq!(card(&cards, &group).summary(), "2 categories");
        assert_eq!(
            card(&cards, &annotate).summary(),
            "Subtasks of every episode"
        );
        assert_eq!(card(&cards, &search).summary(), "“make the first fold”");
        // With another camera the node has read nothing, as the preview says.
        if let CurationNodeKind::BreakDownSubtasks(annotation) =
            payload.node_mut(&annotate).unwrap().kind_mut()
        {
            annotation.set_camera(Some("/camera/wrist".to_owned()));
        }
        let reader = PopulationReader::new(&payload, &sample, &reads);
        let cards = NodeCard::all(&payload, Some(&reader));
        assert_eq!(
            card(&cards, &annotate).summary(),
            "Not run yet · 2 episodes"
        );
    }
}
