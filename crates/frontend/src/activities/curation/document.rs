//! The curation payload: the graph, per-node configuration, and the outputs
//! published as catalog views, encoded as JSON text in
//! `CurationDocument::payload`. It is the recipe only: what a node reads is a
//! preview held for the session (see `preview`). Node positions are not
//! stored; the canvas lays the graph out from its edges every time.
//!
//! The graph's grammar lives here too. Every node hands on a population of
//! one kind — episodes, subtasks, or moments (see `population`) — and what
//! may follow a node is decided by that kind and by the node itself.
//!
//! A node is what it asks, its kind, and what it keeps of the answer, its
//! rule: Score progress keeps the moments progressing fast enough, Find
//! similar the subtasks close enough, and a Select what passes its
//! conditions. The rule is the node's knob, so moving it re-reads nothing the
//! preview holds. Every node that ends in something to train on is an
//! output, and publishes as its own view.
//!
//! This is the contract every Curation view shares. Add fields and helpers;
//! never rename or remove what is here without checking the other views.
mod publication;
mod selection;
#[cfg(test)]
mod tests;

use super::annotate::SubtaskRow;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use publication::PublishedOutput;
pub(crate) use selection::{Comparison, EpisodeCondition, EpisodeFact, Selection, SelectionBasis};

/// Encoding 3 keeps each node's rule beside what it asks, so a Select's
/// conditions and a measure's threshold are the node's own. Encoding 2 kept
/// the thresholds in Selects of their own, and encoding 1 what the nodes
/// measured; neither is converted.
pub(crate) const CURATION_ENCODING_VERSION: u32 = 3;
/// The root node every curation starts with.
pub(crate) const DATASET_NODE_ID: &str = "dataset";
/// OpenRouter id of the newest Gemini Flash as of 2026-09-21; it accepts image
/// input and structured output. Re-check the catalog when a newer one ships.
pub(crate) const DEFAULT_ANNOTATION_MODEL: &str = "google/gemini-3.8-flash";
pub(crate) const DEFAULT_SAMPLE_INTERVAL_SECONDS: f64 = 0.5;
const MAX_NODE_ID_LEN: usize = 64;

/// Everything one curation persists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurationGraph {
    #[serde(default)]
    dataset: Option<DatasetSelection>,
    nodes: Vec<CurationNode>,
    #[serde(default)]
    edges: Vec<CurationEdge>,
    #[serde(default)]
    published: Vec<PublishedOutput>,
}

/// One node: what it asks, and the rule it keeps part of the answer by when
/// its kind keeps by one.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurationNode {
    id: String,
    kind: CurationNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keeps: Option<Selection>,
}
impl CurationNode {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn kind(&self) -> &CurationNodeKind {
        &self.kind
    }
    pub(crate) fn kind_mut(&mut self) -> &mut CurationNodeKind {
        &mut self.kind
    }
    /// The rule the node keeps what it measured by; `None` for a node that
    /// hands on everything it measured.
    pub(crate) fn keeps(&self) -> Option<&Selection> {
        self.keeps.as_ref()
    }
}

/// A parent → child link. Every node except the dataset has exactly one parent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurationEdge {
    from: String,
    to: String,
}
impl CurationEdge {
    pub(crate) fn from(&self) -> &str {
        &self.from
    }
    pub(crate) fn to(&self) -> &str {
        &self.to
    }
}

/// What a node asks, with the configuration its kind needs. The dataset hands
/// on its episodes, a measurement what it measured over its input, and a
/// Select asks nothing: its rule keeps part of its input.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CurationNodeKind {
    Dataset,
    BreakDownSubtasks(SubtaskAnnotation),
    FindSimilarSubtasks(SubtaskSearch),
    GroupSimilarSubtasks(SubtaskGrouping),
    ScoreProgress(ProgressScoring),
    Select,
}
impl CurationNodeKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::BreakDownSubtasks(_) => "break_down_subtasks",
            Self::FindSimilarSubtasks(_) => "find_similar_subtasks",
            Self::GroupSimilarSubtasks(_) => "group_similar_subtasks",
            Self::ScoreProgress(_) => "score_progress",
            Self::Select => "select",
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Dataset => "Dataset",
            Self::BreakDownSubtasks(_) => "Break down subtasks",
            Self::FindSimilarSubtasks(_) => "Find similar subtasks",
            Self::GroupSimilarSubtasks(_) => "Group similar subtasks",
            Self::ScoreProgress(_) => "Score progress",
            Self::Select => "Select",
        }
    }
    /// Whether what the node hands on is something to train on, and so
    /// publishes as a view. Break down's descriptions are free text that
    /// differs from episode to episode until a search or a grouping makes
    /// them one vocabulary, and the dataset is where every curation starts.
    pub(crate) fn is_output(&self) -> bool {
        match self {
            Self::ScoreProgress(_)
            | Self::FindSimilarSubtasks(_)
            | Self::GroupSimilarSubtasks(_)
            | Self::Select => true,
            Self::Dataset | Self::BreakDownSubtasks(_) => false,
        }
    }
    /// What a Select after this node keeps by: the dataset's episode facts,
    /// or a grouping's categories. Every other node keeps by its own rule,
    /// or has nothing to keep by.
    fn select_basis(&self) -> Option<SelectionBasis> {
        match self {
            Self::Dataset => Some(SelectionBasis::EpisodeFacts),
            Self::GroupSimilarSubtasks(_) => Some(SelectionBasis::Categories),
            Self::BreakDownSubtasks(_)
            | Self::FindSimilarSubtasks(_)
            | Self::ScoreProgress(_)
            | Self::Select => None,
        }
    }
    /// What this node's rule keeps by, below a node of kind `parent`; `None`
    /// for a kind that keeps everything it measured.
    fn rule_basis(&self, parent: &Self) -> Option<SelectionBasis> {
        match self {
            Self::ScoreProgress(_) => Some(SelectionBasis::Progress),
            Self::FindSimilarSubtasks(_) => Some(SelectionBasis::Similarity),
            Self::Select => parent.select_basis(),
            Self::Dataset | Self::BreakDownSubtasks(_) | Self::GroupSimilarSubtasks(_) => None,
        }
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::BreakDownSubtasks(annotation) => annotation.validate(),
            Self::GroupSimilarSubtasks(grouping) => grouping.validate(),
            Self::Dataset
            | Self::FindSimilarSubtasks(_)
            | Self::ScoreProgress(_)
            | Self::Select => Ok(()),
        }
    }
}

/// What a node hands on: whole episodes, the subtasks of some, or their
/// moments. It decides what may follow the node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PopulationKind {
    Episodes,
    Subtasks,
    Moments,
}

/// Configuration of one Break down subtasks node.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskAnnotation {
    /// Camera topic to sample; unset until the user picks one.
    #[serde(default)]
    camera: Option<String>,
    /// Fully qualified provider model id.
    model: String,
    sample_interval_seconds: f64,
    /// The topic each episode records the operator's instruction on, read as
    /// the episode's task; unset for a dataset that records none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_topic: Option<String>,
}
impl Default for SubtaskAnnotation {
    fn default() -> Self {
        Self {
            camera: None,
            model: DEFAULT_ANNOTATION_MODEL.to_owned(),
            sample_interval_seconds: DEFAULT_SAMPLE_INTERVAL_SECONDS,
            task_topic: None,
        }
    }
}
impl SubtaskAnnotation {
    pub(crate) fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub(crate) fn set_camera(&mut self, camera: Option<String>) {
        self.camera = camera.filter(|value| !value.trim().is_empty());
    }
    pub(crate) fn model(&self) -> &str {
        &self.model
    }
    pub(crate) fn set_model(&mut self, model: impl Into<String>) -> Result<(), String> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err("The annotation model id is empty.".into());
        }
        self.model = model;
        Ok(())
    }
    pub(crate) fn sample_interval_seconds(&self) -> f64 {
        self.sample_interval_seconds
    }
    pub(crate) fn set_sample_interval_seconds(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err("The sample interval must be a positive number of seconds.".into());
        }
        self.sample_interval_seconds = seconds;
        Ok(())
    }
    pub(crate) fn task_topic(&self) -> Option<&str> {
        self.task_topic.as_deref()
    }
    pub(crate) fn set_task_topic(&mut self, topic: Option<String>) {
        self.task_topic = topic.filter(|value| !value.trim().is_empty());
    }
    fn validate(&self) -> Result<(), String> {
        if self.model.trim().is_empty() {
            return Err("The annotation model id is empty.".into());
        }
        if !self.sample_interval_seconds.is_finite() || self.sample_interval_seconds <= 0.0 {
            return Err("The sample interval must be a positive number of seconds.".into());
        }
        Ok(())
    }
}

/// What one Score progress node asks. Its scorer is pinned, so there is
/// nothing to configure; how fast is fast enough is the node's rule, and
/// what it reads is the preview's, never the document's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProgressScoring {}

/// The query of one Find similar subtasks node, which its step asks about
/// every subtask that reaches it. How close is close enough is the node's
/// rule.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskSearch {
    #[serde(default)]
    query: String,
    /// The row the query was taken from, when it came from one rather than
    /// being typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<SubtaskKey>,
}
impl SubtaskSearch {
    pub(crate) fn query(&self) -> &str {
        &self.query
    }
    pub(crate) fn source(&self) -> Option<&SubtaskKey> {
        self.source.as_ref()
    }
    /// Typed text. Text that is no longer the source row's own words is no
    /// longer a question about that row.
    pub(crate) fn set_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        if query != self.query {
            self.source = None;
        }
        self.query = query;
    }
    /// Ask about one row, in its own words.
    fn ask_about(&mut self, row: &SubtaskRow) {
        self.query = row.description().to_owned();
        self.source = Some(row.key());
    }
}

/// Which row something refers to: its episode and where it starts, which no
/// two subtasks of one episode share.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskKey {
    episode: EpisodeId,
    start_time: f64,
}
impl SubtaskKey {
    pub(crate) fn new(episode: EpisodeId, start_time: f64) -> Self {
        Self {
            episode,
            start_time,
        }
    }
    pub(super) fn is(&self, row: &SubtaskRow) -> bool {
        row.is(&self.episode, self.start_time)
    }
}

/// One category: a short name and the definition the classifier reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskCategory {
    name: String,
    definition: String,
}
impl SubtaskCategory {
    pub(crate) fn new(
        name: impl Into<String>,
        definition: impl Into<String>,
    ) -> Result<Self, String> {
        let category = Self {
            name: name.into().trim().to_owned(),
            definition: definition.into().trim().to_owned(),
        };
        if category.name.is_empty() || category.definition.is_empty() {
            return Err("A category needs a name and a definition.".into());
        }
        Ok(category)
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn definition(&self) -> &str {
        &self.definition
    }
}

/// How many categories one grouping may name: what `spql.ai.classify` takes.
const MAX_CATEGORIES: usize = 255;

/// The categories one Group similar subtasks node puts every subtask that
/// reaches it in. A model proposes them in the browser and the operator may
/// rename or merge them; either way they are the node's configuration, and
/// its step names them in the statement.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskGrouping {
    #[serde(default)]
    categories: Vec<SubtaskCategory>,
}
impl SubtaskGrouping {
    pub(crate) fn categories(&self) -> &[SubtaskCategory] {
        &self.categories
    }
    /// Replace the categories, refusing a list the classifier would.
    pub(crate) fn set_categories(
        &mut self,
        categories: Vec<SubtaskCategory>,
    ) -> Result<(), String> {
        let candidate = Self { categories };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    /// No categories yet, or up to 255 distinct names, each with its
    /// definition.
    fn validate(&self) -> Result<(), String> {
        if self.categories.len() > MAX_CATEGORIES {
            return Err(format!(
                "A grouping names at most {MAX_CATEGORIES} categories."
            ));
        }
        let mut names = BTreeSet::new();
        for category in &self.categories {
            if category.name.trim().is_empty() || category.definition.trim().is_empty() {
                return Err("A category needs a name and a definition.".into());
            }
            if !names.insert(category.name.as_str()) {
                return Err(format!("Category “{}” appears twice.", category.name));
            }
        }
        Ok(())
    }
}

impl CurationGraph {
    /// A graph with only its dataset node.
    pub(crate) fn new(dataset: Option<DatasetSelection>) -> Self {
        Self {
            dataset,
            nodes: vec![CurationNode {
                id: DATASET_NODE_ID.to_owned(),
                kind: CurationNodeKind::Dataset,
                keeps: None,
            }],
            edges: Vec::new(),
            published: Vec::new(),
        }
    }
    pub(crate) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(crate) fn set_dataset(&mut self, dataset: Option<DatasetSelection>) {
        self.dataset = dataset;
    }
    pub(crate) fn nodes(&self) -> &[CurationNode] {
        &self.nodes
    }
    pub(crate) fn edges(&self) -> &[CurationEdge] {
        &self.edges
    }
    /// The camera this graph measures on, out of its annotation nodes; a
    /// graph that has not chosen one leads with whatever shows the workspace.
    pub(super) fn configured_camera(&self) -> crate::activities::episode::FeaturedCamera {
        crate::activities::episode::FeaturedCamera::measured(self.nodes.iter().find_map(|node| {
            match node.kind() {
                CurationNodeKind::BreakDownSubtasks(annotation) => annotation.camera(),
                _ => None,
            }
        }))
    }
    pub(crate) fn node(&self, id: &str) -> Option<&CurationNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
    pub(crate) fn node_mut(&mut self, id: &str) -> Option<&mut CurationNode> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }
    pub(crate) fn parent_of(&self, id: &str) -> Option<&str> {
        self.edges
            .iter()
            .find(|edge| edge.to == id)
            .map(|edge| edge.from.as_str())
    }
    /// Children in creation order.
    pub(crate) fn children_of(&self, id: &str) -> Vec<&CurationNode> {
        self.edges
            .iter()
            .filter(|edge| edge.from == id)
            .filter_map(|edge| self.node(&edge.to))
            .collect()
    }
    /// What `node` hands on. A Select hands on part of what it receives, so
    /// it hands on what its parent does.
    pub(crate) fn population_kind(&self, node: &str) -> Option<PopulationKind> {
        match self.node(node)?.kind() {
            CurationNodeKind::Dataset => Some(PopulationKind::Episodes),
            CurationNodeKind::BreakDownSubtasks(_)
            | CurationNodeKind::FindSimilarSubtasks(_)
            | CurationNodeKind::GroupSimilarSubtasks(_) => Some(PopulationKind::Subtasks),
            CurationNodeKind::ScoreProgress(_) => Some(PopulationKind::Moments),
            CurationNodeKind::Select => self.population_kind(self.parent_of(node)?),
        }
    }
    /// What may follow `node`, in menu order, each with its default
    /// configuration: the measurements that read what it hands on, then a
    /// Select when there is something to select by. A Select holds all its
    /// conditions, so none follows another; and a node that keeps by its own
    /// rule is followed by what reads what it kept.
    pub(crate) fn child_kinds(&self, node: &str) -> Vec<CurationNodeKind> {
        let Some(parent) = self.node(node) else {
            return Vec::new();
        };
        let mut kinds = match (self.population_kind(node), parent.kind()) {
            (Some(PopulationKind::Episodes), _) => vec![
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
                CurationNodeKind::ScoreProgress(ProgressScoring::default()),
            ],
            // Break down the stretches that progress fast enough.
            (Some(PopulationKind::Moments), _) => vec![CurationNodeKind::BreakDownSubtasks(
                SubtaskAnnotation::default(),
            )],
            (
                Some(PopulationKind::Subtasks),
                CurationNodeKind::BreakDownSubtasks(_)
                | CurationNodeKind::FindSimilarSubtasks(_)
                | CurationNodeKind::Select,
            ) => vec![
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
                CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
            ],
            _ => Vec::new(),
        };
        if parent.kind().select_basis().is_some() {
            kinds.push(CurationNodeKind::Select);
        }
        kinds
    }
    fn accepts_child(&self, parent: &str, child: &CurationNodeKind) -> bool {
        self.child_kinds(parent)
            .iter()
            .any(|kind| kind.key() == child.key())
    }
    /// Replace `node`'s rule. The document refuses a rule of another basis,
    /// so a slip can never re-point a node at something else.
    pub(crate) fn set_rule(&mut self, node: &str, rule: Selection) -> Result<(), String> {
        rule.validate()?;
        let target = self
            .node_mut(node)
            .ok_or("That node is no longer in the graph.")?;
        match &mut target.keeps {
            Some(keeps) if keeps.basis() == rule.basis() => {
                *keeps = rule;
                Ok(())
            }
            _ => Err("This node no longer keeps by that measure.".into()),
        }
    }
    /// The Break down subtasks node whose rows reach `node`: the nearest one
    /// above it, through any searches, groupings, and Selects between.
    pub(crate) fn subtask_source(&self, node: &str) -> Option<&str> {
        let mut current = self.parent_of(node)?;
        for _ in 0..self.nodes.len() {
            match self.node(current)?.kind() {
                CurationNodeKind::BreakDownSubtasks(_) => return Some(current),
                CurationNodeKind::FindSimilarSubtasks(_)
                | CurationNodeKind::GroupSimilarSubtasks(_)
                | CurationNodeKind::Select => current = self.parent_of(current)?,
                _ => return None,
            }
        }
        None
    }
    /// The nearest node above `node` that is not a Select: what a Select's
    /// rule is stated in terms of, and what a measurement's preview reads
    /// the output of, through any Select between.
    pub(crate) fn measure_above(&self, node: &str) -> Option<&CurationNode> {
        let mut current = self.parent_of(node)?;
        for _ in 0..self.nodes.len() {
            let node = self.node(current)?;
            match node.kind() {
                CurationNodeKind::Select => current = self.parent_of(current)?,
                _ => return Some(node),
            }
        }
        None
    }
    /// The curation's outputs, in graph order, whatever follows them: each is
    /// one selection of the data to train on, and may be published as a view.
    pub(crate) fn outputs(&self) -> Vec<&CurationNode> {
        self.nodes
            .iter()
            .filter(|node| node.kind().is_output())
            .collect()
    }
    /// Create a child of `parent` and return its id. A kind that keeps by a
    /// rule starts with the rule that keeps everything it measures.
    pub(crate) fn add_child(
        &mut self,
        parent: &str,
        kind: CurationNodeKind,
    ) -> Result<String, String> {
        let parent_node = self.node(parent).ok_or("Unknown parent node.")?;
        if !self.accepts_child(parent, &kind) {
            return Err(format!(
                "{} cannot follow {}.",
                kind.label(),
                parent_node.kind.label()
            ));
        }
        let keeps = kind
            .rule_basis(parent_node.kind())
            .map(Selection::for_basis);
        let id = (1_u32..)
            .map(|index| format!("{}-{index}", kind.key()))
            .find(|candidate| self.node(candidate).is_none())
            .expect("an unused node id exists");
        self.nodes.push(CurationNode {
            id: id.clone(),
            kind,
            keeps,
        });
        self.edges.push(CurationEdge {
            from: parent.to_owned(),
            to: id.clone(),
        });
        Ok(id)
    }
    /// Point the annotation's Find similar node at one of its rows, adding the
    /// node if the annotation has none, and return the node's id. The first
    /// search node is reused, so asking about row after row does not grow the
    /// graph by a node each time.
    pub(super) fn search_for(
        &mut self,
        annotation: &str,
        row: &SubtaskRow,
    ) -> Result<String, String> {
        let existing = self
            .children_of(annotation)
            .into_iter()
            .find(|child| matches!(child.kind, CurationNodeKind::FindSimilarSubtasks(_)))
            .map(|child| child.id.clone());
        let node = match existing {
            Some(node) => node,
            None => self.add_child(
                annotation,
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )?,
        };
        match self.node_mut(&node).map(|node| &mut node.kind) {
            Some(CurationNodeKind::FindSimilarSubtasks(search)) => search.ask_about(row),
            _ => return Err("That node no longer searches subtasks.".into()),
        }
        Ok(node)
    }
    /// Remove a node and its descendants. A branch
    /// holding a published output stays until the view is unpublished, so no
    /// view outlives the recipe it was compiled from.
    pub(crate) fn remove_node(&mut self, id: &str) -> Result<(), String> {
        if id == DATASET_NODE_ID {
            return Err("The dataset node cannot be removed.".into());
        }
        self.node(id).ok_or("Unknown node.")?;
        let mut doomed = vec![id.to_owned()];
        let mut index = 0;
        while index < doomed.len() {
            let current = doomed[index].clone();
            doomed.extend(
                self.children_of(&current)
                    .into_iter()
                    .map(|child| child.id.clone()),
            );
            index += 1;
        }
        if let Some(published) = self
            .published
            .iter()
            .find(|published| doomed.iter().any(|node| node == published.node()))
        {
            return Err(format!(
                "“{}” is published from this branch; unpublish it before removing the step.",
                published.view()
            ));
        }
        self.nodes.retain(|node| !doomed.contains(&node.id));
        self.edges
            .retain(|edge| !doomed.contains(&edge.from) && !doomed.contains(&edge.to));
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.validate_nodes()?;
        self.validate_edges()?;
        self.validate_publications()
    }
    fn validate_nodes(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            if node.id.is_empty()
                || node.id.len() > MAX_NODE_ID_LEN
                || node
                    .id
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(format!("Node id “{}” is not valid.", node.id));
            }
            if !ids.insert(node.id.as_str()) {
                return Err(format!("Node id “{}” appears twice.", node.id));
            }
            node.kind.validate()?;
            if let Some(rule) = &node.keeps {
                rule.validate()?;
            }
        }
        match self.node(DATASET_NODE_ID).map(CurationNode::kind) {
            Some(CurationNodeKind::Dataset) => {}
            _ => return Err("The graph has no dataset node.".into()),
        }
        if self
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, CurationNodeKind::Dataset))
            .count()
            != 1
        {
            return Err("The graph must have exactly one dataset node.".into());
        }
        Ok(())
    }
    /// One parent per node, every node reached from the dataset, every edge
    /// one the grammar allows, and every node keeping by the rule its kind
    /// keeps by there. The grammar is checked last: it walks up through
    /// Selects, which only terminates on a graph without cycles.
    fn validate_edges(&self) -> Result<(), String> {
        let mut parents = BTreeMap::new();
        for edge in &self.edges {
            self.node(&edge.from)
                .ok_or("An edge starts at an unknown node.")?;
            self.node(&edge.to)
                .ok_or("An edge ends at an unknown node.")?;
            if edge.from == edge.to {
                return Err("A node cannot follow itself.".into());
            }
            if parents
                .insert(edge.to.as_str(), edge.from.as_str())
                .is_some()
            {
                return Err(format!("Node “{}” has two parents.", edge.to));
            }
        }
        if parents.contains_key(DATASET_NODE_ID) {
            return Err("The dataset node cannot have a parent.".into());
        }
        for node in &self.nodes {
            if node.id != DATASET_NODE_ID && !parents.contains_key(node.id.as_str()) {
                return Err(format!("Node “{}” is not connected.", node.id));
            }
        }
        let mut reached = BTreeSet::from([DATASET_NODE_ID]);
        let mut frontier = vec![DATASET_NODE_ID];
        while let Some(current) = frontier.pop() {
            for child in self.children_of(current) {
                if reached.insert(child.id.as_str()) {
                    frontier.push(child.id.as_str());
                }
            }
        }
        if reached.len() != self.nodes.len() {
            return Err("The graph contains a cycle.".into());
        }
        for edge in &self.edges {
            let (Some(from), Some(to)) = (self.node(&edge.from), self.node(&edge.to)) else {
                continue;
            };
            if !self.accepts_child(&edge.from, &to.kind) {
                return Err(format!(
                    "{} cannot follow {}.",
                    to.kind.label(),
                    from.kind.label()
                ));
            }
        }
        for node in &self.nodes {
            let basis = self
                .parent_of(&node.id)
                .and_then(|parent| self.node(parent))
                .and_then(|parent| node.kind.rule_basis(parent.kind()));
            if node.keeps.as_ref().map(Selection::basis) != basis {
                return Err(format!(
                    "{} “{}” keeps by a rule its place does not allow.",
                    node.kind.label(),
                    node.id
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn encode(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| format!("encoding the curation graph: {error}"))
    }
    pub(crate) fn decode(encoding: u32, payload: &str) -> Result<Self, String> {
        if encoding != CURATION_ENCODING_VERSION {
            return Err(format!(
                "This curation uses encoding {encoding}; this Studio reads encoding {CURATION_ENCODING_VERSION}."
            ));
        }
        let decoded: Self = serde_json::from_str(payload).map_err(|error| {
            format!("The saved curation graph has an unsupported shape: {error}")
        })?;
        decoded.validate()?;
        Ok(decoded)
    }
}

#[cfg(test)]
impl CurationGraph {
    /// Create a child of `parent` keeping by `rule` rather than the rule its
    /// kind starts with, and return its id.
    pub(crate) fn add_keeping(
        &mut self,
        parent: &str,
        kind: CurationNodeKind,
        rule: Selection,
    ) -> Result<String, String> {
        let id = self.add_child(parent, kind)?;
        self.set_rule(&id, rule)?;
        Ok(id)
    }
}
