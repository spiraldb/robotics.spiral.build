//! What one node of the Build graph says, and what its + offers. Everything
//! here reads the payload and nothing else, so the canvas keeps no second
//! copy of the graph.
use std::fmt::{Display, Formatter, Result as FormatResult};

use spiral_robotics_common::datasets::DatasetSelection;

use super::super::document::{BuildNode, BuildNodeKind, BuildPayload, DATASET_NODE_ID};
use super::super::state::BuildController;

/// A count with the noun a person would say beside it: "1 episode",
/// "12 episodes".
struct Counted(usize, Noun);

impl Display for Counted {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FormatResult {
        let Noun(singular, plural) = self.1;
        let word = if self.0 == 1 { singular } else { plural };
        write!(formatter, "{} {word}", self.0)
    }
}

/// A word in both its numbers, because English does not make the plural by
/// adding an `s`: "categorys" told an operator the numbers beside it were not
/// being read by anyone.
#[derive(Clone, Copy, Debug)]
struct Noun(&'static str, &'static str);

impl Noun {
    const EPISODE: Self = Self("episode", "episodes");
    const SUBTASK: Self = Self("subtask", "subtasks");
    const CATEGORY: Self = Self("category", "categories");
}

/// What a node does to the data; drives its glyph and identity tint, in the
/// same vocabulary the Workflow graph and the Curation pipeline speak.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BuildNodeFace {
    Dataset,
    Annotation,
    Progress,
    Search,
    Grouping,
}

impl BuildNodeFace {
    pub(super) fn of(kind: &BuildNodeKind) -> Self {
        match kind {
            BuildNodeKind::Dataset => Self::Dataset,
            BuildNodeKind::BreakDownSubtasks(_) => Self::Annotation,
            BuildNodeKind::FindUnproductiveMoments => Self::Progress,
            BuildNodeKind::FindSimilarSubtasks(_) => Self::Search,
            BuildNodeKind::GroupSimilarSubtasks => Self::Grouping,
        }
    }
    pub(super) const fn class_name(self) -> &'static str {
        match self {
            Self::Dataset => "workflow-stage-source",
            Self::Annotation => "workflow-stage-model",
            Self::Progress => "workflow-stage-shape",
            Self::Search => "workflow-stage-media",
            Self::Grouping => "workflow-stage-join",
        }
    }
}

/// What the + on a node offers, and what choosing one does. A node whose
/// output supports nothing has no menu and draws no +.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BuildChildMenu {
    parent: String,
    kinds: Vec<BuildNodeKind>,
}

impl BuildChildMenu {
    fn of(node: &BuildNode) -> Option<Self> {
        let kinds = node.kind().child_kinds();
        (!kinds.is_empty()).then(|| Self {
            parent: node.id().to_owned(),
            kinds,
        })
    }
    pub(super) fn labels(&self) -> Vec<&'static str> {
        self.kinds.iter().map(BuildNodeKind::label).collect()
    }
    /// Add the chosen child to the graph and select it, so the workspace
    /// opens on the node the user just created.
    pub(super) fn choose(
        &self,
        controller: BuildController,
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
pub(super) struct BuildNodeCard {
    id: String,
    face: BuildNodeFace,
    label: &'static str,
    summary: String,
    executable: bool,
    menu: Option<BuildChildMenu>,
    order: usize,
}

impl BuildNodeCard {
    /// Every node of the graph, in payload order.
    pub(super) fn all(payload: &BuildPayload) -> Vec<Self> {
        payload
            .nodes()
            .iter()
            .map(|node| Self::new(payload, node))
            .collect()
    }
    fn new(payload: &BuildPayload, node: &BuildNode) -> Self {
        Self {
            id: node.id().to_owned(),
            face: BuildNodeFace::of(node.kind()),
            label: node.kind().label(),
            summary: Self::describe(payload, node),
            executable: node.kind().executable(),
            menu: BuildChildMenu::of(node),
            order: 0,
        }
    }
    /// One line saying what this node holds right now.
    fn describe(payload: &BuildPayload, node: &BuildNode) -> String {
        match node.kind() {
            BuildNodeKind::Dataset => payload
                .dataset()
                .map_or_else(|| "Choose a dataset".to_owned(), DatasetSelection::label),
            BuildNodeKind::BreakDownSubtasks(annotation) => {
                let measured = format!(
                    "{} · {}",
                    Counted(annotation.completed_episodes().len(), Noun::EPISODE),
                    Counted(payload.subtasks_for(node.id()).count(), Noun::SUBTASK),
                );
                match annotation.camera() {
                    Some(camera) => format!("{measured} · {camera}"),
                    None => measured,
                }
            }
            BuildNodeKind::FindUnproductiveMoments => "Planned".to_owned(),
            BuildNodeKind::FindSimilarSubtasks(search) => {
                let query = search.query().trim();
                if query.is_empty() {
                    "No query yet".to_owned()
                } else {
                    format!("“{query}”")
                }
            }
            BuildNodeKind::GroupSimilarSubtasks => payload.grouping_for(node.id()).map_or_else(
                || "No categories yet".to_owned(),
                |grouping| Counted(grouping.categories().len(), Noun::CATEGORY).to_string(),
            ),
        }
    }
    pub(super) fn with_order(mut self, order: usize) -> Self {
        self.order = order;
        self
    }
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn face(&self) -> BuildNodeFace {
        self.face
    }
    pub(super) fn label(&self) -> &'static str {
        self.label
    }
    pub(super) fn summary(&self) -> &str {
        &self.summary
    }
    /// Whether Studio can run this node today; a planned one is dimmed.
    pub(super) fn executable(&self) -> bool {
        self.executable
    }
    pub(super) fn menu(&self) -> Option<&BuildChildMenu> {
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

    fn card<'a>(cards: &'a [BuildNodeCard], id: &str) -> &'a BuildNodeCard {
        cards.iter().find(|card| card.id() == id).expect("card")
    }

    #[test]
    fn a_new_graph_asks_for_a_dataset_and_offers_the_measurements() {
        let payload = BuildPayload::new(None);
        let cards = BuildNodeCard::all(&payload);
        let dataset = card(&cards, DATASET_NODE_ID);
        assert_eq!(dataset.summary(), "Choose a dataset");
        assert_eq!(dataset.face(), BuildNodeFace::Dataset);
        assert!(!dataset.removable());
        assert!(dataset.executable());
        assert_eq!(
            dataset
                .menu()
                .expect("the dataset accepts children")
                .labels(),
            ["Break down subtasks", "Find unproductive moments"]
        );
    }

    #[test]
    fn each_kind_says_what_it_holds_in_one_line() {
        let mut payload =
            BuildPayload::new(DatasetSelection::new(vec!["cloth_fold_demo".to_owned()]).ok());
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                BuildNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        payload
            .replace_episode_subtasks(
                &annotate,
                "ep-1",
                vec![SubtaskRow::new(&annotate, "ep-1", 0.0, 8.0, "grasp").unwrap()],
            )
            .unwrap();
        let planned = payload
            .add_child(DATASET_NODE_ID, BuildNodeKind::FindUnproductiveMoments)
            .unwrap();
        let search = payload
            .add_child(
                &annotate,
                BuildNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        let group = payload
            .add_child(&annotate, BuildNodeKind::GroupSimilarSubtasks)
            .unwrap();

        let cards = BuildNodeCard::all(&payload);
        assert_eq!(card(&cards, DATASET_NODE_ID).summary(), "cloth_fold_demo");
        assert_eq!(card(&cards, &annotate).summary(), "1 episode · 1 subtask");
        assert_eq!(card(&cards, &planned).summary(), "Planned");
        assert!(!card(&cards, &planned).executable());
        assert!(card(&cards, &planned).menu().is_none());
        assert_eq!(card(&cards, &search).summary(), "No query yet");
        assert_eq!(card(&cards, &group).summary(), "No categories yet");
        assert_eq!(
            card(&cards, &annotate).menu().expect("menu").labels(),
            ["Find similar subtasks", "Group similar subtasks"]
        );
        assert!(card(&cards, &annotate).removable());

        if let BuildNodeKind::BreakDownSubtasks(annotation) =
            payload.node_mut(&annotate).unwrap().kind_mut()
        {
            annotation.set_camera(Some("/camera/top".to_owned()));
        }
        if let BuildNodeKind::FindSimilarSubtasks(config) =
            payload.node_mut(&search).unwrap().kind_mut()
        {
            config.set_query("make the first fold");
        }
        let cards = BuildNodeCard::all(&payload);
        assert_eq!(
            card(&cards, &annotate).summary(),
            "1 episode · 1 subtask · /camera/top"
        );
        assert_eq!(card(&cards, &search).summary(), "“make the first fold”");
    }
}
