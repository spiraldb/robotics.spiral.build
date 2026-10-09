//! The curation payload: the graph, per-node configuration, and the nodes
//! published as catalog views, encoded as JSON text in
//! `CurationDocument::payload`. It is the recipe only: what a node reads is a
//! preview held for the session, restored from the answers stored with the
//! dataset when the curation opens (see `preview`). Node positions are not
//! stored; the canvas lays the graph out from its edges every time.
//!
//! The graph's grammar lives here too. Every edge carries the same table, one
//! row per episode with the time ranges it keeps of it: a model node adds
//! columns to it, and a Select keeps part of it by conditions on the columns
//! above it (`selection`). So anything may follow anything, except that a
//! judgment of subtasks needs a Break down above it. Every node but the
//! dataset may be published as its own view.
//!
//! This is the contract every Curation view shares. Add fields and helpers;
//! never rename or remove what is here without checking the other views.
mod grouping;
mod publication;
mod selection;
#[cfg(test)]
mod tests;

use super::annotate::SubtaskRow;
use crate::activities::episode::{EpisodeListing, EpisodeTask};
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_common::sql::UnquotedSqlIdentifier;
use spiral_robotics_common::training::PublishedModel;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use grouping::{CategoryId, SubtaskCategory, SubtaskGrouping};
pub(crate) use publication::PublishedOutput;
pub(crate) use selection::{
    Comparison, EpisodeCondition, EpisodeFact, EpisodeNumber, SelectCondition, Selection,
};

/// The payload's encoding. A payload of any other encoding is refused, not
/// converted.
pub(crate) const CURATION_ENCODING_VERSION: u32 = 5;
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
    /// The dataset it reads, and the producer that made it, which decide
    /// how its episodes are listed.
    #[serde(default)]
    dataset: Option<EpisodeListing>,
    nodes: Vec<CurationNode>,
    #[serde(default)]
    edges: Vec<CurationEdge>,
    #[serde(default)]
    published: Vec<PublishedOutput>,
}

/// One node: its id, which also names the columns it adds, and what it is.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurationNode {
    id: String,
    kind: CurationNodeKind,
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
    /// The name the node's columns and steps take in a statement: its id as
    /// one plain identifier (`track_objects-1` is `track_objects_1`).
    pub(crate) fn stem(&self) -> String {
        self.id.replace('-', "_")
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

/// What a node is, with the configuration its kind needs. The dataset starts
/// the table, a model node adds what it asks a model about each episode, and
/// a Select keeps part of the table by its conditions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CurationNodeKind {
    Dataset,
    BreakDownSubtasks(SubtaskAnnotation),
    FindSimilarSubtasks(SubtaskSearch),
    GroupSimilarSubtasks(SubtaskGrouping),
    ScoreProgress(ProgressScoring),
    TrackObjects(ObjectTracking),
    Select(Selection),
}
impl CurationNodeKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::BreakDownSubtasks(_) => "break_down_subtasks",
            Self::FindSimilarSubtasks(_) => "find_similar_subtasks",
            Self::GroupSimilarSubtasks(_) => "group_similar_subtasks",
            Self::ScoreProgress(_) => "score_progress",
            Self::TrackObjects(_) => "track_objects",
            Self::Select(_) => "select",
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Dataset => "Dataset",
            Self::BreakDownSubtasks(_) => "Break down subtasks",
            Self::FindSimilarSubtasks(_) => "Find similar subtasks",
            Self::GroupSimilarSubtasks(_) => "Group similar subtasks",
            Self::ScoreProgress(_) => "Score progress",
            Self::TrackObjects(_) => "Track objects",
            Self::Select(_) => "Select",
        }
    }
    /// Whether the node asks a model about each episode and adds what it
    /// answered as columns.
    pub(crate) fn is_model(&self) -> bool {
        !matches!(self, Self::Dataset | Self::Select(_))
    }
    /// Whether a judgment of subtasks, which reads the Break down above it.
    fn judges_subtasks(&self) -> bool {
        matches!(
            self,
            Self::FindSimilarSubtasks(_) | Self::GroupSimilarSubtasks(_)
        )
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::BreakDownSubtasks(annotation) => annotation.validate(),
            Self::GroupSimilarSubtasks(grouping) => grouping.validate(),
            Self::TrackObjects(tracking) => tracking.validate(),
            Self::Select(selection) => selection.validate(),
            Self::Dataset | Self::FindSimilarSubtasks(_) | Self::ScoreProgress(_) => Ok(()),
        }
    }
}

/// Configuration of one Break down subtasks node.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskAnnotation {
    /// Camera topic to sample; unset until the user picks one.
    #[serde(default)]
    camera: Option<String>,
    sample_interval_seconds: f64,
    /// The topic each episode records the operator's instruction on, read as
    /// the episode's task; unset for a dataset that records none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_topic: Option<String>,
    /// Whether each episode's task is the one its dataset lists beside it,
    /// as a LeRobot dataset does; never with a task topic.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    listed_task: bool,
}
impl Default for SubtaskAnnotation {
    fn default() -> Self {
        Self {
            camera: None,
            sample_interval_seconds: DEFAULT_SAMPLE_INTERVAL_SECONDS,
            task_topic: None,
            listed_task: false,
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
    /// Where each episode's task is read; none for a dataset that records
    /// none.
    pub(crate) fn task(&self) -> Option<EpisodeTask> {
        match (&self.task_topic, self.listed_task) {
            (Some(topic), _) => Some(EpisodeTask::Recorded(topic.clone())),
            (None, true) => Some(EpisodeTask::Listed),
            (None, false) => None,
        }
    }
    pub(crate) fn set_task(&mut self, task: Option<EpisodeTask>) {
        (self.task_topic, self.listed_task) = match task {
            Some(EpisodeTask::Recorded(topic)) if !topic.trim().is_empty() => (Some(topic), false),
            Some(EpisodeTask::Listed) => (None, true),
            Some(EpisodeTask::Recorded(_)) | None => (None, false),
        };
    }
    fn validate(&self) -> Result<(), String> {
        if !self.sample_interval_seconds.is_finite() || self.sample_interval_seconds <= 0.0 {
            return Err("The sample interval must be a positive number of seconds.".into());
        }
        if self.task_topic.is_some() && self.listed_task {
            return Err("An episode's task is read from one place.".into());
        }
        Ok(())
    }
}

/// What one Score progress node asks: the progress model it scores with,
/// the released one unless it names a published one. What it reads is the
/// preview's, never the document's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProgressScoring {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published: Option<PublishedProgressModel>,
}
impl ProgressScoring {
    /// The published model it scores with; `None` scores with the released
    /// one.
    pub(crate) fn published(&self) -> Option<&PublishedProgressModel> {
        self.published.as_ref()
    }
    pub(crate) fn set_published(&mut self, published: Option<PublishedProgressModel>) {
        self.published = published;
    }
}

/// A published progress model as a Score progress node names it: the
/// selector the scorer reads its weights by, and the camera it was trained
/// on. The weights fix both, so neither goes stale; the name it is listed by
/// is the project's, and never stored here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishedProgressModel {
    model: String,
    camera: String,
}
impl From<&PublishedModel> for PublishedProgressModel {
    fn from(published: &PublishedModel) -> Self {
        Self {
            model: published.model().to_owned(),
            camera: published.camera().to_owned(),
        }
    }
}
impl PublishedProgressModel {
    pub(crate) fn model(&self) -> &str {
        &self.model
    }
    pub(crate) fn camera(&self) -> &str {
        &self.camera
    }
}

/// What one Track objects node asks: what to track, on which camera, and how
/// many pictures a second the tracker samples.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectTracking {
    /// Camera topic to track on; unset until one is chosen.
    #[serde(default)]
    camera: Option<String>,
    /// What to track, as a short noun phrase such as "bottle"; empty until
    /// one is named.
    #[serde(default)]
    prompt: String,
    /// Pictures a second the tracker samples.
    rate: u32,
}
impl Default for ObjectTracking {
    fn default() -> Self {
        Self {
            camera: None,
            prompt: String::new(),
            rate: Self::DEFAULT_RATE,
        }
    }
}
impl ObjectTracking {
    /// Five pictures a second follows a hand's work without tracking every
    /// frame.
    pub(crate) const DEFAULT_RATE: u32 = 5;
    /// The most pictures a second: every picture of a 30 Hz camera.
    pub(crate) const MAX_RATE: u32 = 30;

    pub(crate) fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub(crate) fn set_camera(&mut self, camera: Option<String>) {
        self.camera = camera.filter(|value| !value.trim().is_empty());
    }
    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }
    pub(crate) fn set_prompt(&mut self, prompt: &str) {
        prompt.trim().clone_into(&mut self.prompt);
    }
    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }
    pub(crate) fn set_rate(&mut self, rate: u32) -> Result<(), String> {
        let candidate = Self {
            rate,
            ..self.clone()
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    /// What it tracks, several of them: "bottles", "boxes", "cherries".
    pub(crate) fn plural(&self) -> String {
        let prompt = match self.prompt.as_str() {
            "" => "object",
            prompt => prompt,
        };
        let consonant_y = prompt.ends_with('y')
            && !prompt
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|before| "aeiou".contains(before));
        if consonant_y {
            format!("{}ies", &prompt[..prompt.len() - 1])
        } else if ["s", "x", "z", "ch", "sh"]
            .iter()
            .any(|ending| prompt.ends_with(ending))
        {
            format!("{prompt}es")
        } else {
            format!("{prompt}s")
        }
    }
    fn validate(&self) -> Result<(), String> {
        if !(1..=Self::MAX_RATE).contains(&self.rate) {
            return Err(format!("Track 1 to {} pictures a second.", Self::MAX_RATE));
        }
        Ok(())
    }
}

/// The query of one Find similar subtasks node, which it asks about every
/// subtask of the Break down above it.
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

impl CurationGraph {
    /// A graph with only its dataset node.
    pub(crate) fn new(dataset: Option<EpisodeListing>) -> Self {
        Self {
            dataset,
            nodes: vec![CurationNode {
                id: DATASET_NODE_ID.to_owned(),
                kind: CurationNodeKind::Dataset,
            }],
            edges: Vec::new(),
            published: Vec::new(),
        }
    }
    pub(crate) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref().map(EpisodeListing::dataset)
    }
    /// How the dataset it reads lists its episodes.
    pub(crate) fn listing(&self) -> Option<&EpisodeListing> {
        self.dataset.as_ref()
    }
    pub(crate) fn set_dataset(&mut self, dataset: Option<EpisodeListing>) {
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
                CurationNodeKind::TrackObjects(tracking) => tracking.camera(),
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
    /// The nodes above `id`, its parent first and the dataset last. A graph
    /// with a cycle stops after every node.
    pub(crate) fn ancestors(&self, id: &str) -> Vec<&CurationNode> {
        let mut above = Vec::new();
        let mut current = self.parent_of(id);
        while let Some(parent) = current.and_then(|parent| self.node(parent)) {
            if above.len() > self.nodes.len() {
                break;
            }
            above.push(parent);
            current = self.parent_of(parent.id());
        }
        above
    }
    /// What may follow `node`, in menu order, each with its default
    /// configuration: the model nodes, the judgments of subtasks once a Break
    /// down is at or above it, and a Select.
    pub(crate) fn child_kinds(&self, node: &str) -> Vec<CurationNodeKind> {
        let Some(parent) = self.node(node) else {
            return Vec::new();
        };
        let mut kinds = vec![
            CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            CurationNodeKind::ScoreProgress(ProgressScoring::default()),
            CurationNodeKind::TrackObjects(ObjectTracking::default()),
        ];
        let described = matches!(parent.kind(), CurationNodeKind::BreakDownSubtasks(_))
            || self.breakdown_above(node).is_some();
        if described {
            kinds.extend([
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
                CurationNodeKind::GroupSimilarSubtasks(SubtaskGrouping::default()),
            ]);
        }
        kinds.push(CurationNodeKind::Select(Selection::keeping_everything()));
        kinds
    }
    fn accepts_child(&self, parent: &str, child: &CurationNodeKind) -> bool {
        self.child_kinds(parent)
            .iter()
            .any(|kind| kind.key() == child.key())
    }
    /// The Break down subtasks node whose subtasks `node` judges: the nearest
    /// one above it.
    pub(crate) fn subtask_source(&self, node: &str) -> Option<&str> {
        self.breakdown_above(node).map(CurationNode::id)
    }
    fn breakdown_above(&self, node: &str) -> Option<&CurationNode> {
        self.ancestors(node)
            .into_iter()
            .find(|above| matches!(above.kind(), CurationNodeKind::BreakDownSubtasks(_)))
    }
    /// The nearest node above `node` that is not a Select: the model node, or
    /// the dataset, whose reads decide what reaches it.
    pub(crate) fn measure_above(&self, node: &str) -> Option<&CurationNode> {
        self.ancestors(node)
            .into_iter()
            .find(|above| !matches!(above.kind(), CurationNodeKind::Select(_)))
    }
    /// The model nodes whose nearest node above, through any Select between,
    /// is `node`, in graph order: those whose previews read once `node`'s
    /// have.
    pub(crate) fn measures_below(&self, node: &str) -> Vec<&CurationNode> {
        self.nodes
            .iter()
            .filter(|below| {
                below.kind().is_model()
                    && self
                        .measure_above(below.id())
                        .is_some_and(|above| above.id() == node)
            })
            .collect()
    }
    /// The nodes that may be published: every one but the dataset.
    pub(crate) fn outputs(&self) -> Vec<&CurationNode> {
        self.nodes
            .iter()
            .filter(|node| !matches!(node.kind(), CurationNodeKind::Dataset))
            .collect()
    }
    /// Create a child of `parent` and return its id.
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
        let id = (1_u32..)
            .map(|index| format!("{}-{index}", kind.key()))
            .find(|candidate| self.node(candidate).is_none())
            .expect("an unused node id exists");
        self.nodes.push(CurationNode {
            id: id.clone(),
            kind,
        });
        self.edges.push(CurationEdge {
            from: parent.to_owned(),
            to: id.clone(),
        });
        Ok(id)
    }
    /// Replace a Select's conditions, refusing ones that read what is not
    /// above it.
    pub(crate) fn set_selection(&mut self, node: &str, selection: Selection) -> Result<(), String> {
        let mut candidate = self.clone();
        match candidate.node_mut(node).map(CurationNode::kind_mut) {
            Some(CurationNodeKind::Select(current)) => *current = selection,
            Some(_) => return Err("That node does not select.".into()),
            None => return Err("That node is no longer in the graph.".into()),
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
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
    /// `id` and every node below it, `id` first.
    pub(crate) fn branch_of(&self, id: &str) -> Vec<String> {
        let mut branch = vec![id.to_owned()];
        let mut index = 0;
        while index < branch.len() {
            let current = branch[index].clone();
            branch.extend(
                self.children_of(&current)
                    .into_iter()
                    .map(|child| child.id.clone()),
            );
            index += 1;
        }
        branch
    }
    /// Remove a node and its descendants. A branch holding a published node
    /// stays until the view is unpublished, so no view outlives the recipe it
    /// was compiled from.
    pub(crate) fn remove_node(&mut self, id: &str) -> Result<(), String> {
        if id == DATASET_NODE_ID {
            return Err("The dataset node cannot be removed.".into());
        }
        self.node(id).ok_or("Unknown node.")?;
        let doomed = self.branch_of(id);
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
        self.validate_selections()?;
        self.validate_publications()
    }
    fn validate_nodes(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut stems = BTreeSet::new();
        for node in &self.nodes {
            if node.id.is_empty()
                || node.id.len() > MAX_NODE_ID_LEN
                || UnquotedSqlIdentifier::try_from(node.stem().as_str()).is_err()
            {
                return Err(format!("Node id “{}” is not valid.", node.id));
            }
            if !ids.insert(node.id.as_str()) || !stems.insert(node.stem()) {
                return Err(format!("Node id “{}” appears twice.", node.id));
            }
            node.kind.validate()?;
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
    /// One parent per node, every node reached from the dataset, and every
    /// edge one the grammar allows. The grammar is checked last: it walks up
    /// the graph, which only terminates on a graph without cycles.
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
            if to.kind.judges_subtasks() && self.breakdown_above(&to.id).is_none() {
                return Err(format!(
                    "{} cannot follow {}: it judges the subtasks of a Break down above it.",
                    to.kind.label(),
                    from.kind.label()
                ));
            }
        }
        Ok(())
    }
    /// Every Select's conditions read nodes above it, of the kind each
    /// condition reads.
    fn validate_selections(&self) -> Result<(), String> {
        for node in &self.nodes {
            if let CurationNodeKind::Select(selection) = node.kind() {
                selection.validate_against(&self.ancestors(&node.id))?;
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
