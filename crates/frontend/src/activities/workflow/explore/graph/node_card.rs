//! What one node of the exploration graph says, and what its + offers. Everything
//! here reads the payload and nothing else, so the canvas keeps no second
//! copy of the graph.
use spiral_robotics_common::datasets::DatasetSelection;

use super::super::document::{
    DATASET_NODE_ID, ExplorationNode, ExplorationNodeKind, ExplorationPayload,
};
use super::super::review::Counted;
use super::super::state::ExplorationController;

/// What a node does to the data; drives its glyph and identity tint, in the
/// same vocabulary the Workflow graph and the Curation pipeline speak.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExploreNodeFace {
    Dataset,
    Annotation,
    Search,
    Grouping,
}

impl ExploreNodeFace {
    pub(super) fn of(kind: &ExplorationNodeKind) -> Self {
        match kind {
            ExplorationNodeKind::Dataset => Self::Dataset,
            ExplorationNodeKind::BreakDownSubtasks(_) => Self::Annotation,
            ExplorationNodeKind::FindSimilarSubtasks(_) => Self::Search,
            ExplorationNodeKind::GroupSimilarSubtasks => Self::Grouping,
        }
    }
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Dataset => "workflow-stage-source",
            Self::Annotation => "workflow-stage-model",
            Self::Search => "workflow-stage-media",
            Self::Grouping => "workflow-stage-join",
        }
    }
}

/// What the + on a node offers, and what choosing one does. A node whose
/// output supports nothing has no menu and draws no +.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExploreChildMenu {
    parent: String,
    kinds: Vec<ExplorationNodeKind>,
}

impl ExploreChildMenu {
    fn of(node: &ExplorationNode) -> Option<Self> {
        let kinds = node.kind().child_kinds();
        (!kinds.is_empty()).then(|| Self {
            parent: node.id().to_owned(),
            kinds,
        })
    }
    pub(super) fn labels(&self) -> Vec<&'static str> {
        self.kinds.iter().map(ExplorationNodeKind::label).collect()
    }
    /// Add the chosen child to the graph and select it, so the workspace
    /// opens on the node the user just created.
    pub(super) fn choose(
        &self,
        controller: ExplorationController,
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
/// and what may follow it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExploreNodeCard {
    id: String,
    face: ExploreNodeFace,
    label: &'static str,
    summary: String,
    menu: Option<ExploreChildMenu>,
    order: usize,
}

impl ExploreNodeCard {
    /// Every node of the graph, in payload order.
    pub(super) fn all(payload: &ExplorationPayload) -> Vec<Self> {
        payload
            .nodes()
            .iter()
            .map(|node| Self::new(payload, node))
            .collect()
    }
    fn new(payload: &ExplorationPayload, node: &ExplorationNode) -> Self {
        Self {
            id: node.id().to_owned(),
            face: ExploreNodeFace::of(node.kind()),
            label: node.kind().label(),
            summary: Self::describe(payload, node),
            menu: ExploreChildMenu::of(node),
            order: 0,
        }
    }
    /// One line saying what this node holds right now.
    fn describe(payload: &ExplorationPayload, node: &ExplorationNode) -> String {
        match node.kind() {
            // Every card's line is what the node holds, never a second copy of
            // the instruction its open workspace is already giving.
            ExplorationNodeKind::Dataset => payload
                .dataset()
                .map_or_else(|| "No dataset yet".to_owned(), DatasetSelection::label),
            ExplorationNodeKind::BreakDownSubtasks(annotation) => {
                let measured = format!(
                    "{} · {}",
                    Counted::episodes(annotation.completed_episodes().len()),
                    Counted::subtasks(payload.subtasks_for(node.id()).count()),
                );
                match annotation.camera() {
                    Some(camera) => format!("{measured} · {camera}"),
                    None => measured,
                }
            }
            ExplorationNodeKind::FindSimilarSubtasks(search) => {
                let query = search.query().trim();
                if query.is_empty() {
                    "No query yet".to_owned()
                } else {
                    format!("“{query}”")
                }
            }
            ExplorationNodeKind::GroupSimilarSubtasks => {
                payload.grouping_for(node.id()).map_or_else(
                    || "No categories yet".to_owned(),
                    |grouping| Counted::categories(grouping.categories().len()).to_string(),
                )
            }
        }
    }
    pub(super) fn with_order(mut self, order: usize) -> Self {
        self.order = order;
        self
    }
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn face(&self) -> ExploreNodeFace {
        self.face
    }
    pub(super) fn label(&self) -> &'static str {
        self.label
    }
    pub(super) fn summary(&self) -> &str {
        &self.summary
    }
    pub(super) fn menu(&self) -> Option<&ExploreChildMenu> {
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
    use super::super::super::document::{SubtaskAnnotation, SubtaskRow, SubtaskSearch};
    use super::*;

    fn card<'a>(cards: &'a [ExploreNodeCard], id: &str) -> &'a ExploreNodeCard {
        cards.iter().find(|card| card.id() == id).expect("card")
    }

    #[test]
    fn a_new_graph_asks_for_a_dataset_and_offers_the_measurements() {
        let payload = ExplorationPayload::new(None);
        let cards = ExploreNodeCard::all(&payload);
        let dataset = card(&cards, DATASET_NODE_ID);
        assert_eq!(dataset.summary(), "No dataset yet");
        assert_eq!(dataset.face(), ExploreNodeFace::Dataset);
        assert!(!dataset.removable());
        assert_eq!(
            dataset
                .menu()
                .expect("the dataset accepts children")
                .labels(),
            ["Break down subtasks"]
        );
    }

    #[test]
    fn each_kind_says_what_it_holds_in_one_line() {
        let mut payload =
            ExplorationPayload::new(DatasetSelection::new(vec!["cloth_fold_demo".to_owned()]).ok());
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        payload
            .replace_episode_subtasks(
                &annotate,
                "ep-1",
                vec![SubtaskRow::new(&annotate, "ep-1", 0.0, 8.0, "grasp").unwrap()],
            )
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                ExplorationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(&annotate, ExplorationNodeKind::GroupSimilarSubtasks)
            .unwrap();

        let cards = ExploreNodeCard::all(&payload);
        assert_eq!(card(&cards, DATASET_NODE_ID).summary(), "cloth_fold_demo");
        assert_eq!(card(&cards, &annotate).summary(), "1 episode · 1 subtask");
        assert!(card(&cards, &group).menu().is_none());
        assert_eq!(card(&cards, &search).summary(), "No query yet");
        assert_eq!(card(&cards, &group).summary(), "No categories yet");
        assert_eq!(
            card(&cards, &annotate).menu().expect("menu").labels(),
            ["Find similar subtasks", "Group similar subtasks"]
        );
        assert!(card(&cards, &annotate).removable());

        if let ExplorationNodeKind::BreakDownSubtasks(annotation) =
            payload.node_mut(&annotate).unwrap().kind_mut()
        {
            annotation.set_camera(Some("/camera/top".to_owned()));
        }
        if let ExplorationNodeKind::FindSimilarSubtasks(config) =
            payload.node_mut(&search).unwrap().kind_mut()
        {
            config.set_query("make the first fold");
        }
        let cards = ExploreNodeCard::all(&payload);
        assert_eq!(
            card(&cards, &annotate).summary(),
            "1 episode · 1 subtask · /camera/top"
        );
        assert_eq!(card(&cards, &search).summary(), "“make the first fold”");
    }
}
