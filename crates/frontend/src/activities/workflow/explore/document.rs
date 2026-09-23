//! The exploration payload: the graph, per-node configuration, and measurement
//! outputs one exploration persists, encoded as JSON text in
//! `ExplorationDocument::payload`. Node positions are not stored; the canvas lays
//! the graph out from its edges every time.
//!
//! This is the contract every Explore view shares. Add fields and helpers;
//! never rename or remove what is here without checking the other views.
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const EXPLORATION_ENCODING_VERSION: u32 = 1;
/// The root node every exploration starts with.
pub(crate) const DATASET_NODE_ID: &str = "dataset";
/// OpenRouter id of the newest Gemini Flash as of 2026-09-21; it accepts image
/// input and structured output. Re-check the catalog when a newer one ships.
pub(crate) const DEFAULT_ANNOTATION_MODEL: &str = "google/gemini-3.8-flash";
pub(crate) const DEFAULT_SAMPLE_INTERVAL_SECONDS: f64 = 0.5;
const MAX_NODE_ID_LEN: usize = 64;

/// Everything one exploration persists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplorationPayload {
    #[serde(default)]
    dataset: Option<DatasetSelection>,
    nodes: Vec<ExplorationNode>,
    #[serde(default)]
    edges: Vec<ExplorationEdge>,
    #[serde(default)]
    subtasks: Vec<SubtaskRow>,
    #[serde(default)]
    groupings: Vec<SubtaskGrouping>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplorationNode {
    id: String,
    kind: ExplorationNodeKind,
}
impl ExplorationNode {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn kind(&self) -> &ExplorationNodeKind {
        &self.kind
    }
    pub(crate) fn kind_mut(&mut self) -> &mut ExplorationNodeKind {
        &mut self.kind
    }
}

/// A parent → child link. Every node except the dataset has exactly one parent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplorationEdge {
    from: String,
    to: String,
}
impl ExplorationEdge {
    pub(crate) fn from(&self) -> &str {
        &self.from
    }
    pub(crate) fn to(&self) -> &str {
        &self.to
    }
}

/// What a node is, with the configuration its kind needs. Measurements read
/// the dataset; explorations read a measurement's saved output.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ExplorationNodeKind {
    Dataset,
    BreakDownSubtasks(SubtaskAnnotation),
    FindSimilarSubtasks(SubtaskSearch),
    GroupSimilarSubtasks,
}
impl ExplorationNodeKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::BreakDownSubtasks(_) => "break_down_subtasks",
            Self::FindSimilarSubtasks(_) => "find_similar_subtasks",
            Self::GroupSimilarSubtasks => "group_similar_subtasks",
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Dataset => "Dataset",
            Self::BreakDownSubtasks(_) => "Break down subtasks",
            Self::FindSimilarSubtasks(_) => "Find similar subtasks",
            Self::GroupSimilarSubtasks => "Group similar subtasks",
        }
    }
    /// The child kinds this node's + menu offers, in menu order, each with
    /// its default configuration.
    pub(crate) fn child_kinds(&self) -> Vec<Self> {
        match self {
            Self::Dataset => vec![Self::BreakDownSubtasks(SubtaskAnnotation::default())],
            Self::BreakDownSubtasks(_) => vec![
                Self::FindSimilarSubtasks(SubtaskSearch::default()),
                Self::GroupSimilarSubtasks,
            ],
            Self::FindSimilarSubtasks(_) | Self::GroupSimilarSubtasks => Vec::new(),
        }
    }
    fn accepts_child(&self, child: &Self) -> bool {
        self.child_kinds()
            .iter()
            .any(|kind| kind.key() == child.key())
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::BreakDownSubtasks(annotation) => annotation.validate(),
            Self::Dataset | Self::FindSimilarSubtasks(_) | Self::GroupSimilarSubtasks => Ok(()),
        }
    }
}

/// Configuration and progress of one Break down subtasks node.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskAnnotation {
    /// Camera topic to sample; unset until the user picks one.
    #[serde(default)]
    camera: Option<String>,
    /// Fully qualified provider model id.
    model: String,
    sample_interval_seconds: f64,
    /// Episodes whose rows are persisted, including episodes with no subtasks.
    #[serde(default)]
    completed_episodes: Vec<EpisodeId>,
    /// Episodes whose last annotation attempt failed; a rerun replaces the entry.
    #[serde(default)]
    failed_episodes: Vec<EpisodeFailure>,
}
impl Default for SubtaskAnnotation {
    fn default() -> Self {
        Self {
            camera: None,
            model: DEFAULT_ANNOTATION_MODEL.to_owned(),
            sample_interval_seconds: DEFAULT_SAMPLE_INTERVAL_SECONDS,
            completed_episodes: Vec::new(),
            failed_episodes: Vec::new(),
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
    pub(crate) fn completed_episodes(&self) -> &[EpisodeId] {
        &self.completed_episodes
    }
    pub(crate) fn failed_episodes(&self) -> &[EpisodeFailure] {
        &self.failed_episodes
    }
    pub(crate) fn is_completed(&self, episode: &EpisodeId) -> bool {
        self.completed_episodes.contains(episode)
    }
    fn mark_completed(&mut self, episode: &EpisodeId) {
        if !self.is_completed(episode) {
            self.completed_episodes.push(episode.clone());
        }
        self.failed_episodes
            .retain(|failure| &failure.episode != episode);
    }
    fn mark_failed(&mut self, episode: &EpisodeId, reason: impl Into<String>) {
        self.failed_episodes
            .retain(|failure| &failure.episode != episode);
        self.failed_episodes.push(EpisodeFailure {
            episode: episode.clone(),
            reason: reason.into(),
        });
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpisodeFailure {
    episode: EpisodeId,
    reason: String,
}
impl EpisodeFailure {
    pub(crate) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

/// The last query of a Find similar subtasks node. Results are never stored.
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
        self.query = row.description.clone();
        self.source = Some(row.key());
    }
}

/// Which row something refers to: its episode and where it starts, which no
/// two rows of one annotation share.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskKey {
    episode: EpisodeId,
    start_time: f64,
}

/// One subtask interval of one episode, produced by one annotation node.
/// `start_time` is inclusive and `end_time` exclusive, in seconds from the
/// start of the episode.
///
/// The row carries the still an index shows it as. The annotation pass has
/// already decoded that picture — it is one of the frames it drew onto the
/// contact sheets the model read — so cutting it there costs nothing and saves
/// every later reader a decode. It is optional because rows annotated before
/// posters were stored have none; those are cut on demand instead.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskRow {
    node: String,
    episode: EpisodeId,
    start_time: f64,
    end_time: f64,
    description: String,
    /// A `data:image/jpeg;base64,…` still of this interval's middle, about
    /// 5.5 kB: 311 of them are 1.7 MB, five per cent of what one exploration
    /// may carry (`MAX_EXPLORATION_PAYLOAD_BYTES`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poster: Option<String>,
}
impl SubtaskRow {
    pub(crate) fn new(
        node: impl Into<String>,
        episode: EpisodeId,
        start_time: f64,
        end_time: f64,
        description: impl Into<String>,
    ) -> Result<Self, String> {
        let row = Self {
            node: node.into(),
            episode,
            start_time,
            end_time,
            description: description.into().trim().to_owned(),
            poster: None,
        };
        row.validate()?;
        Ok(row)
    }
    /// The still this row shows, as the annotation pass cut it. An empty
    /// picture is no picture.
    pub(crate) fn with_poster(mut self, poster: String) -> Self {
        self.poster = Some(poster).filter(|poster| !poster.is_empty());
        self
    }
    pub(crate) fn poster(&self) -> Option<&str> {
        self.poster.as_deref()
    }
    /// Where in the interval its poster is cut from. The middle shows the
    /// action under way rather than the reach into it or the release out of
    /// it, which is what tells two similar subtasks apart.
    pub(crate) fn midpoint(&self) -> f64 {
        f64::midpoint(self.start_time, self.end_time)
    }
    #[cfg(test)]
    pub(crate) fn node(&self) -> &str {
        &self.node
    }
    pub(crate) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(crate) fn start_time(&self) -> f64 {
        self.start_time
    }
    pub(crate) fn end_time(&self) -> f64 {
        self.end_time
    }
    #[cfg(test)]
    pub(crate) fn duration(&self) -> f64 {
        self.end_time - self.start_time
    }
    pub(crate) fn description(&self) -> &str {
        &self.description
    }
    /// Whether this is the row an assignment or focus refers to.
    pub(crate) fn is(&self, episode: &EpisodeId, start_time: f64) -> bool {
        &self.episode == episode && self.start_time == start_time
    }
    pub(crate) fn key(&self) -> SubtaskKey {
        SubtaskKey {
            episode: self.episode.clone(),
            start_time: self.start_time,
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.node.is_empty() {
            return Err("A subtask row needs its node.".into());
        }
        if !self.start_time.is_finite()
            || !self.end_time.is_finite()
            || self.start_time < 0.0
            || self.end_time <= self.start_time
        {
            return Err(format!(
                "Subtask interval [{}, {}) is not a valid time range.",
                self.start_time, self.end_time
            ));
        }
        if self.description.trim().is_empty() {
            return Err("A subtask description is empty.".into());
        }
        Ok(())
    }
}

/// One LLM-proposed category: a short name and the definition jev reads.
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

/// jev's Choice answer for one subtask row: the winning category, its
/// probability, the distribution's confidence, and the category it would
/// otherwise have joined.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskAssignment {
    episode: EpisodeId,
    start_time: f64,
    category: String,
    probability: f64,
    confidence: f64,
    /// Absent for groupings assigned before runner-ups were kept, and for a
    /// grouping of one category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runner_up: Option<RunnerUp>,
}

/// The second most probable category of one row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunnerUp {
    category: String,
    probability: f64,
}
impl RunnerUp {
    pub(crate) fn category(&self) -> &str {
        &self.category
    }
}
impl SubtaskAssignment {
    pub(crate) fn new(
        episode: EpisodeId,
        start_time: f64,
        category: impl Into<String>,
        probability: f64,
        confidence: f64,
    ) -> Result<Self, String> {
        let assignment = Self {
            episode,
            start_time,
            category: category.into(),
            probability,
            confidence,
            runner_up: None,
        };
        assignment.validate()?;
        Ok(assignment)
    }
    /// The category this row came second in.
    pub(crate) fn with_runner_up(
        mut self,
        category: impl Into<String>,
        probability: f64,
    ) -> Result<Self, String> {
        self.runner_up = Some(RunnerUp {
            category: category.into(),
            probability,
        });
        self.validate()?;
        Ok(self)
    }
    pub(crate) fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub(crate) fn start_time(&self) -> f64 {
        self.start_time
    }
    pub(crate) fn category(&self) -> &str {
        &self.category
    }
    pub(crate) fn probability(&self) -> f64 {
        self.probability
    }
    pub(crate) fn confidence(&self) -> f64 {
        self.confidence
    }
    pub(crate) fn runner_up(&self) -> Option<&RunnerUp> {
        self.runner_up.as_ref()
    }
    fn validate(&self) -> Result<(), String> {
        let unit = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
        if !self.start_time.is_finite() || self.category.is_empty() {
            return Err("A subtask assignment needs its row and category.".into());
        }
        if !unit(self.probability) || !unit(self.confidence) {
            return Err("Assignment probability and confidence must be between 0 and 1.".into());
        }
        if let Some(runner_up) = &self.runner_up {
            if runner_up.category.is_empty() || runner_up.category == self.category {
                return Err("A runner-up must be a different category.".into());
            }
            if !unit(runner_up.probability) {
                return Err("A runner-up probability must be between 0 and 1.".into());
            }
        }
        Ok(())
    }
}

/// The saved output of one Group similar subtasks node over its parent
/// annotation's rows.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskGrouping {
    node: String,
    /// The annotation node whose rows were grouped.
    source: String,
    categories: Vec<SubtaskCategory>,
    assignments: Vec<SubtaskAssignment>,
}
impl SubtaskGrouping {
    pub(crate) fn new(
        node: impl Into<String>,
        source: impl Into<String>,
        categories: Vec<SubtaskCategory>,
        assignments: Vec<SubtaskAssignment>,
    ) -> Result<Self, String> {
        let grouping = Self {
            node: node.into(),
            source: source.into(),
            categories,
            assignments,
        };
        grouping.validate()?;
        Ok(grouping)
    }
    pub(crate) fn categories(&self) -> &[SubtaskCategory] {
        &self.categories
    }
    pub(crate) fn assignments(&self) -> &[SubtaskAssignment] {
        &self.assignments
    }
    /// A category's rows, most typical first.
    pub(crate) fn members(&self, category: &str) -> Vec<&SubtaskAssignment> {
        let mut members = self
            .assignments
            .iter()
            .filter(|assignment| assignment.category == category)
            .collect::<Vec<_>>();
        members.sort_by(|left, right| right.probability.total_cmp(&left.probability));
        members
    }
    fn validate(&self) -> Result<(), String> {
        if self.node.is_empty() || self.source.is_empty() {
            return Err("A grouping needs its node and source annotation.".into());
        }
        if self.categories.is_empty() {
            return Err("A grouping needs at least one category.".into());
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
        for assignment in &self.assignments {
            assignment.validate()?;
            let runner_up = assignment.runner_up.as_ref().map(RunnerUp::category);
            for category in std::iter::once(assignment.category.as_str()).chain(runner_up) {
                if !names.contains(category) {
                    return Err(format!(
                        "Assignment refers to unknown category “{category}”."
                    ));
                }
            }
        }
        Ok(())
    }
}

impl ExplorationPayload {
    /// A graph with only its dataset node.
    pub(crate) fn new(dataset: Option<DatasetSelection>) -> Self {
        Self {
            dataset,
            nodes: vec![ExplorationNode {
                id: DATASET_NODE_ID.to_owned(),
                kind: ExplorationNodeKind::Dataset,
            }],
            edges: Vec::new(),
            subtasks: Vec::new(),
            groupings: Vec::new(),
        }
    }
    pub(crate) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(crate) fn set_dataset(&mut self, dataset: Option<DatasetSelection>) {
        self.dataset = dataset;
    }
    pub(crate) fn nodes(&self) -> &[ExplorationNode] {
        &self.nodes
    }
    pub(crate) fn edges(&self) -> &[ExplorationEdge] {
        &self.edges
    }
    /// The camera this graph measures on, out of its annotation nodes; a
    /// graph that has not chosen one leads with whatever shows the workspace.
    pub(crate) fn configured_camera(&self) -> crate::activities::episode::FeaturedCamera {
        crate::activities::episode::FeaturedCamera::measured(self.nodes.iter().find_map(|node| {
            match node.kind() {
                ExplorationNodeKind::BreakDownSubtasks(annotation) => annotation.camera(),
                _ => None,
            }
        }))
    }
    pub(crate) fn node(&self, id: &str) -> Option<&ExplorationNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
    pub(crate) fn node_mut(&mut self, id: &str) -> Option<&mut ExplorationNode> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }
    pub(crate) fn parent_of(&self, id: &str) -> Option<&str> {
        self.edges
            .iter()
            .find(|edge| edge.to == id)
            .map(|edge| edge.from.as_str())
    }
    /// Children in creation order.
    pub(crate) fn children_of(&self, id: &str) -> Vec<&ExplorationNode> {
        self.edges
            .iter()
            .filter(|edge| edge.from == id)
            .filter_map(|edge| self.node(&edge.to))
            .collect()
    }
    /// Create a child of `parent` and return its id.
    pub(crate) fn add_child(
        &mut self,
        parent: &str,
        kind: ExplorationNodeKind,
    ) -> Result<String, String> {
        let parent_node = self.node(parent).ok_or("Unknown parent node.")?;
        if !parent_node.kind.accepts_child(&kind) {
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
        self.nodes.push(ExplorationNode {
            id: id.clone(),
            kind,
        });
        self.edges.push(ExplorationEdge {
            from: parent.to_owned(),
            to: id.clone(),
        });
        Ok(id)
    }
    /// Point the annotation's Find similar node at one of its rows, adding the
    /// node if the annotation has none, and return the node's id. The first
    /// search node is reused, so asking about row after row does not grow the
    /// graph by a node each time.
    pub(crate) fn search_for(
        &mut self,
        annotation: &str,
        row: &SubtaskRow,
    ) -> Result<String, String> {
        if row.node != annotation {
            return Err("That subtask belongs to another annotation.".into());
        }
        let existing = self
            .children_of(annotation)
            .into_iter()
            .find(|child| matches!(child.kind, ExplorationNodeKind::FindSimilarSubtasks(_)))
            .map(|child| child.id.clone());
        let node = match existing {
            Some(node) => node,
            None => self.add_child(
                annotation,
                ExplorationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )?,
        };
        match self.node_mut(&node).map(|node| &mut node.kind) {
            Some(ExplorationNodeKind::FindSimilarSubtasks(search)) => search.ask_about(row),
            _ => return Err("That node no longer searches subtasks.".into()),
        }
        Ok(node)
    }
    /// Remove a node, its descendants, and everything they produced.
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
        self.nodes.retain(|node| !doomed.contains(&node.id));
        self.edges
            .retain(|edge| !doomed.contains(&edge.from) && !doomed.contains(&edge.to));
        self.subtasks.retain(|row| !doomed.contains(&row.node));
        self.groupings
            .retain(|grouping| !doomed.contains(&grouping.node));
        Ok(())
    }
    /// An annotation node's rows, in episode order then time order as stored.
    pub(crate) fn subtasks_for<'a>(
        &'a self,
        node: &'a str,
    ) -> impl Iterator<Item = &'a SubtaskRow> + 'a {
        self.subtasks.iter().filter(move |row| row.node == node)
    }
    /// Replace one episode's rows for an annotation node and record the
    /// episode as completed. Rows may arrive in any order; they must not
    /// overlap, and an empty list records an episode with no subtasks.
    pub(crate) fn replace_episode_subtasks(
        &mut self,
        node: &str,
        episode: &EpisodeId,
        mut rows: Vec<SubtaskRow>,
    ) -> Result<(), String> {
        let annotation = self.annotation_mut(node)?;
        if rows
            .iter()
            .any(|row| row.node != node || &row.episode != episode)
        {
            return Err("Subtask rows belong to another node or episode.".into());
        }
        for row in &rows {
            row.validate()?;
        }
        rows.sort_by(|left, right| left.start_time.total_cmp(&right.start_time));
        Self::check_sequence(&rows)?;
        annotation.mark_completed(episode);
        self.subtasks
            .retain(|row| !(row.node == node && &row.episode == episode));
        self.subtasks.extend(rows);
        Ok(())
    }
    /// Record a failed annotation attempt; earlier rows for the episode stay.
    pub(crate) fn record_episode_failure(
        &mut self,
        node: &str,
        episode: &EpisodeId,
        reason: impl Into<String>,
    ) -> Result<(), String> {
        self.annotation_mut(node)?.mark_failed(episode, reason);
        Ok(())
    }
    fn annotation_mut(&mut self, node: &str) -> Result<&mut SubtaskAnnotation, String> {
        match self.node_mut(node).map(|node| &mut node.kind) {
            Some(ExplorationNodeKind::BreakDownSubtasks(annotation)) => Ok(annotation),
            Some(_) => Err("That node does not annotate subtasks.".into()),
            None => Err("Unknown node.".into()),
        }
    }
    fn check_sequence(sorted: &[SubtaskRow]) -> Result<(), String> {
        for pair in sorted.windows(2) {
            if pair[1].start_time < pair[0].end_time {
                return Err(format!(
                    "Subtasks [{}, {}) and [{}, {}) overlap.",
                    pair[0].start_time, pair[0].end_time, pair[1].start_time, pair[1].end_time
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn grouping_for(&self, node: &str) -> Option<&SubtaskGrouping> {
        self.groupings.iter().find(|grouping| grouping.node == node)
    }
    /// Store a grouping node's output, replacing any earlier output.
    pub(crate) fn set_grouping(&mut self, grouping: SubtaskGrouping) -> Result<(), String> {
        grouping.validate()?;
        match self.node(&grouping.node).map(ExplorationNode::kind) {
            Some(ExplorationNodeKind::GroupSimilarSubtasks) => {}
            Some(_) => return Err("That node does not group subtasks.".into()),
            None => return Err("Unknown node.".into()),
        }
        if self.source_annotation(&grouping.node) != Some(grouping.source.as_str()) {
            return Err("A grouping's source must be its parent annotation.".into());
        }
        self.groupings
            .retain(|existing| existing.node != grouping.node);
        self.groupings.push(grouping);
        Ok(())
    }
    /// The annotation node an exploration reads: its parent, when that parent
    /// is a Break down subtasks node.
    pub(crate) fn source_annotation(&self, node: &str) -> Option<&str> {
        let parent = self.parent_of(node)?;
        matches!(
            self.node(parent).map(ExplorationNode::kind),
            Some(ExplorationNodeKind::BreakDownSubtasks(_))
        )
        .then_some(parent)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
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
        }
        match self.node(DATASET_NODE_ID).map(ExplorationNode::kind) {
            Some(ExplorationNodeKind::Dataset) => {}
            _ => return Err("The graph has no dataset node.".into()),
        }
        if self
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, ExplorationNodeKind::Dataset))
            .count()
            != 1
        {
            return Err("The graph must have exactly one dataset node.".into());
        }
        let mut parents = BTreeMap::new();
        for edge in &self.edges {
            let from = self
                .node(&edge.from)
                .ok_or("An edge starts at an unknown node.")?;
            let to = self
                .node(&edge.to)
                .ok_or("An edge ends at an unknown node.")?;
            if edge.from == edge.to {
                return Err("A node cannot follow itself.".into());
            }
            if !from.kind.accepts_child(&to.kind) {
                return Err(format!(
                    "{} cannot follow {}.",
                    to.kind.label(),
                    from.kind.label()
                ));
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
        let mut by_episode: BTreeMap<(&str, &EpisodeId), Vec<&SubtaskRow>> = BTreeMap::new();
        for row in &self.subtasks {
            row.validate()?;
            match self.node(&row.node).map(ExplorationNode::kind) {
                Some(ExplorationNodeKind::BreakDownSubtasks(_)) => {}
                _ => {
                    return Err(format!(
                        "Subtask rows refer to unknown node “{}”.",
                        row.node
                    ));
                }
            }
            by_episode
                .entry((row.node.as_str(), &row.episode))
                .or_default()
                .push(row);
        }
        for rows in by_episode.values() {
            for pair in rows.windows(2) {
                if pair[1].start_time < pair[0].end_time {
                    return Err(format!(
                        "Subtasks of episode {} are out of order or overlap.",
                        pair[0].episode
                    ));
                }
            }
        }
        let mut grouped = BTreeSet::new();
        for grouping in &self.groupings {
            grouping.validate()?;
            if !grouped.insert(grouping.node.as_str()) {
                return Err(format!("Node “{}” has two groupings.", grouping.node));
            }
            match self.node(&grouping.node).map(ExplorationNode::kind) {
                Some(ExplorationNodeKind::GroupSimilarSubtasks) => {}
                _ => {
                    return Err(format!(
                        "Grouping refers to unknown node “{}”.",
                        grouping.node
                    ));
                }
            }
            if self.source_annotation(&grouping.node) != Some(grouping.source.as_str()) {
                return Err(format!(
                    "Grouping “{}” does not read its parent annotation.",
                    grouping.node
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn encode(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self)
            .map_err(|error| format!("encoding the exploration graph: {error}"))
    }
    pub(crate) fn decode(encoding: u32, payload: &str) -> Result<Self, String> {
        if encoding != EXPLORATION_ENCODING_VERSION {
            return Err(format!(
                "This exploration uses encoding {encoding}; this Studio reads {EXPLORATION_ENCODING_VERSION}."
            ));
        }
        let decoded: Self = serde_json::from_str(payload).map_err(|error| {
            format!("The saved exploration graph has an unsupported shape: {error}")
        })?;
        decoded.validate()?;
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    fn annotated_graph() -> (ExplorationPayload, String) {
        let mut payload = ExplorationPayload::new(None);
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        payload
            .replace_episode_subtasks(
                &annotate,
                &EpisodeId::fixture(1),
                vec![
                    SubtaskRow::new(
                        &annotate,
                        EpisodeId::fixture(1),
                        8.0,
                        13.0,
                        "make the first fold",
                    )
                    .unwrap(),
                    SubtaskRow::new(
                        &annotate,
                        EpisodeId::fixture(1),
                        0.0,
                        8.0,
                        " grasp the corners ",
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        (payload, annotate)
    }

    #[test]
    fn a_fresh_graph_is_one_dataset_node_that_validates() {
        let payload = ExplorationPayload::new(None);
        assert_eq!(payload.validate(), Ok(()));
        assert_eq!(payload.nodes().len(), 1);
        assert_eq!(
            payload.node(DATASET_NODE_ID).unwrap().kind().label(),
            "Dataset"
        );
        assert_eq!(payload.parent_of(DATASET_NODE_ID), None);
    }

    #[test]
    fn children_follow_the_menu_and_get_unique_ids() {
        let mut payload = ExplorationPayload::new(None);
        let first = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let second = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("break_down_subtasks-1", "break_down_subtasks-2")
        );
        let search = payload
            .add_child(
                &first,
                ExplorationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        assert_eq!(payload.parent_of(&search), Some(first.as_str()));
        assert_eq!(payload.source_annotation(&search), Some(first.as_str()));
        assert!(
            payload
                .add_child(DATASET_NODE_ID, ExplorationNodeKind::GroupSimilarSubtasks)
                .is_err()
        );
        assert!(
            payload
                .add_child(&search, ExplorationNodeKind::GroupSimilarSubtasks)
                .is_err()
        );
        assert!(
            payload
                .add_child("missing", ExplorationNodeKind::GroupSimilarSubtasks)
                .is_err()
        );
        assert_eq!(payload.children_of(DATASET_NODE_ID).len(), 2);
        assert_eq!(payload.validate(), Ok(()));
    }

    #[test]
    fn episode_rows_are_sorted_trimmed_and_recorded_as_completed() {
        let (payload, annotate) = annotated_graph();
        let rows = payload
            .subtasks_for(&annotate)
            .filter(|row| row.episode() == &EpisodeId::fixture(1))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].description(), "grasp the corners");
        assert_eq!(rows[1].start_time(), 8.0);
        assert_eq!(rows[1].duration(), 5.0);
        let ExplorationNodeKind::BreakDownSubtasks(annotation) =
            payload.node(&annotate).unwrap().kind()
        else {
            panic!("annotation node");
        };
        assert!(annotation.is_completed(&EpisodeId::fixture(1)));
        assert_eq!(payload.validate(), Ok(()));
    }

    #[test]
    fn rows_that_overlap_or_belong_elsewhere_are_rejected_before_any_change() {
        let (mut payload, annotate) = annotated_graph();
        let before = payload.clone();
        let overlapping = vec![
            SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 5.0, "a").unwrap(),
            SubtaskRow::new(&annotate, EpisodeId::fixture(2), 4.0, 9.0, "b").unwrap(),
        ];
        assert!(
            payload
                .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), overlapping)
                .is_err()
        );
        let foreign = vec![SubtaskRow::new("other", EpisodeId::fixture(2), 0.0, 5.0, "a").unwrap()];
        assert!(
            payload
                .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), foreign)
                .is_err()
        );
        assert!(
            payload
                .replace_episode_subtasks(DATASET_NODE_ID, &EpisodeId::fixture(2), Vec::new())
                .is_err()
        );
        assert_eq!(payload, before);
        assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), 5.0, 5.0, "a").is_err());
        assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), -1.0, 5.0, "a").is_err());
        assert!(SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 5.0, "  ").is_err());
    }

    #[test]
    fn an_empty_episode_and_a_failure_both_record_progress() {
        let (mut payload, annotate) = annotated_graph();
        payload
            .replace_episode_subtasks(&annotate, &EpisodeId::fixture(2), Vec::new())
            .unwrap();
        payload
            .record_episode_failure(&annotate, &EpisodeId::fixture(3), "timed out")
            .unwrap();
        let ExplorationNodeKind::BreakDownSubtasks(annotation) =
            payload.node(&annotate).unwrap().kind()
        else {
            panic!("annotation node");
        };
        assert_eq!(
            annotation.completed_episodes(),
            [EpisodeId::fixture(1), EpisodeId::fixture(2)]
        );
        assert_eq!(
            annotation.failed_episodes()[0].episode(),
            &EpisodeId::fixture(3)
        );
        assert_eq!(annotation.failed_episodes()[0].reason(), "timed out");
        payload
            .replace_episode_subtasks(&annotate, &EpisodeId::fixture(3), Vec::new())
            .unwrap();
        let ExplorationNodeKind::BreakDownSubtasks(annotation) =
            payload.node(&annotate).unwrap().kind()
        else {
            panic!("annotation node");
        };
        assert!(annotation.failed_episodes().is_empty());
    }

    #[test]
    fn a_rows_poster_round_trips_and_an_older_row_decodes_without_one() {
        let (mut payload, annotate) = annotated_graph();
        let poster = format!("data:image/jpeg;base64,{}", "A".repeat(32));
        payload
            .replace_episode_subtasks(
                &annotate,
                &EpisodeId::fixture(2),
                vec![
                    SubtaskRow::new(&annotate, EpisodeId::fixture(2), 0.0, 6.0, "unfold")
                        .unwrap()
                        .with_poster(poster.clone()),
                ],
            )
            .unwrap();
        let encoded = payload.encode().unwrap();
        let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        // Only the row that has one carries the field at all.
        assert_eq!(json["subtasks"][0].get("poster"), None);
        assert_eq!(json["subtasks"][2]["poster"], poster);
        let decoded = ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, &encoded).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(
            decoded
                .subtasks_for(&annotate)
                .find(|row| row.episode() == &EpisodeId::fixture(2))
                .unwrap()
                .poster(),
            Some(poster.as_str())
        );
        // An exploration saved before posters existed still decodes under
        // `deny_unknown_fields`, and its rows simply have none.
        let older = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"}],"subtasks":[{"node":"a","episode":"00000000000000000000000000000001","start_time":0.0,"end_time":4.0,"description":"grasp"}]}"#;
        let older = ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, older).unwrap();
        let row = older.subtasks_for("a").next().unwrap();
        assert_eq!(row.poster(), None);
        assert_eq!(row.midpoint(), 2.0);
        // An empty picture is no picture, so nothing is stored for it.
        assert_eq!(row.clone().with_poster(String::new()).poster(), None);
    }

    #[test]
    fn a_whole_datasets_posters_fit_in_one_exploration() {
        // What the sample dataset actually produced: 39 episodes, 311 rows.
        // A poster is about 5.5 kB at 192 pixels wide and JPEG quality 0.7.
        let poster = format!("data:image/jpeg;base64,{}", "A".repeat(5_500));
        let mut payload = ExplorationPayload::new(None);
        let annotate = payload
            .add_child(
                DATASET_NODE_ID,
                ExplorationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
            )
            .unwrap();
        let rows = (0..311)
            .map(|index| {
                SubtaskRow::new(
                    &annotate,
                    EpisodeId::fixture(1),
                    f64::from(index),
                    f64::from(index) + 1.0,
                    "fold",
                )
                .unwrap()
                .with_poster(poster.clone())
            })
            .collect();
        payload
            .replace_episode_subtasks(&annotate, &EpisodeId::fixture(1), rows)
            .unwrap();
        let encoded = payload.encode().unwrap().len();
        assert!(
            encoded * 10 < spiral_robotics_common::exploration::MAX_EXPLORATION_PAYLOAD_BYTES,
            "a whole dataset's posters are {encoded} bytes, more than a tenth of what an \
             exploration may carry"
        );
    }

    #[test]
    fn groupings_read_their_parent_and_order_members_by_probability() {
        let (mut payload, annotate) = annotated_graph();
        let group = payload
            .add_child(&annotate, ExplorationNodeKind::GroupSimilarSubtasks)
            .unwrap();
        let categories = vec![
            SubtaskCategory::new("Grasp", "Take hold of the cloth").unwrap(),
            SubtaskCategory::new("Fold", "Bring one part over another").unwrap(),
        ];
        let assignments = vec![
            SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Grasp", 0.9, 0.8).unwrap(),
            SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.7, 0.4).unwrap(),
            SubtaskAssignment::new(EpisodeId::fixture(9), 1.0, "Fold", 0.95, 0.9).unwrap(),
        ];
        let grouping =
            SubtaskGrouping::new(&group, &annotate, categories.clone(), assignments.clone())
                .unwrap();
        payload.set_grouping(grouping).unwrap();
        let members = payload.grouping_for(&group).unwrap().members("Fold");
        assert_eq!(
            members
                .iter()
                .map(|member| member.episode().clone())
                .collect::<Vec<_>>(),
            [EpisodeId::fixture(9), EpisodeId::fixture(1)]
        );
        assert!(
            SubtaskGrouping::new(
                &group,
                &annotate,
                categories.clone(),
                vec![
                    SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Smooth", 0.5, 0.5).unwrap()
                ]
            )
            .is_err()
        );
        assert!(SubtaskGrouping::new(&group, "elsewhere", categories.clone(), Vec::new()).is_ok());
        assert!(
            payload
                .set_grouping(
                    SubtaskGrouping::new(&group, "elsewhere", categories.clone(), Vec::new())
                        .unwrap()
                )
                .is_err()
        );
        assert!(
            payload
                .set_grouping(
                    SubtaskGrouping::new(&annotate, &annotate, categories, Vec::new()).unwrap()
                )
                .is_err()
        );
        assert!(SubtaskAssignment::new(EpisodeId::fixture(1), 0.0, "Grasp", 1.2, 0.5).is_err());
        assert_eq!(payload.validate(), Ok(()));
    }

    #[test]
    fn a_runner_up_must_be_another_known_category_and_older_assignments_have_none() {
        let (mut payload, annotate) = annotated_graph();
        let group = payload
            .add_child(&annotate, ExplorationNodeKind::GroupSimilarSubtasks)
            .unwrap();
        let categories = vec![
            SubtaskCategory::new("Grasp", "Take hold of the cloth").unwrap(),
            SubtaskCategory::new("Fold", "Bring one part over another").unwrap(),
        ];
        let assigned = SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
            .unwrap()
            .with_runner_up("Grasp", 0.35)
            .unwrap();
        assert_eq!(assigned.runner_up().map(RunnerUp::category), Some("Grasp"));
        let unknown = SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
            .unwrap()
            .with_runner_up("Smooth", 0.35)
            .unwrap();
        assert!(
            SubtaskGrouping::new(&group, &annotate, categories.clone(), vec![unknown]).is_err()
        );
        assert!(
            SubtaskAssignment::new(EpisodeId::fixture(1), 8.0, "Fold", 0.6, 0.3)
                .unwrap()
                .with_runner_up("Fold", 0.35)
                .is_err()
        );
        payload
            .set_grouping(
                SubtaskGrouping::new(&group, &annotate, categories, vec![assigned]).unwrap(),
            )
            .unwrap();

        let older: SubtaskAssignment = serde_json::from_str(
            r#"{"episode":"00000000000000000000000000000001","start_time":0,
                "category":"Grasp","probability":0.9,"confidence":0.8}"#,
        )
        .unwrap();
        assert!(older.runner_up().is_none());
        assert!(!serde_json::to_string(&older).unwrap().contains("runner_up"));
    }

    #[test]
    fn asking_about_a_row_reuses_the_first_search_node_and_remembers_the_row() {
        let (mut payload, annotate) = annotated_graph();
        let rows = payload.subtasks_for(&annotate).cloned().collect::<Vec<_>>();
        let first = payload.search_for(&annotate, &rows[0]).unwrap();
        let again = payload.search_for(&annotate, &rows[1]).unwrap();
        assert_eq!(first, again);
        assert_eq!(payload.children_of(&annotate).len(), 1);
        let ExplorationNodeKind::FindSimilarSubtasks(search) = payload.node(&first).unwrap().kind()
        else {
            panic!("a search node");
        };
        assert_eq!(search.query(), "make the first fold");
        assert_eq!(search.source(), Some(&rows[1].key()));

        let mut typed = search.clone();
        typed.set_query("make the first fold");
        assert_eq!(typed.source(), Some(&rows[1].key()));
        typed.set_query("smooth the cloth");
        assert_eq!(typed.source(), None);

        let elsewhere = SubtaskRow::new("other", EpisodeId::fixture(1), 0.0, 1.0, "grasp").unwrap();
        assert!(payload.search_for(&annotate, &elsewhere).is_err());
        assert_eq!(payload.validate(), Ok(()));
    }

    #[test]
    fn removing_a_node_takes_its_subtree_and_outputs_with_it() {
        let (mut payload, annotate) = annotated_graph();
        let group = payload
            .add_child(&annotate, ExplorationNodeKind::GroupSimilarSubtasks)
            .unwrap();
        payload
            .set_grouping(
                SubtaskGrouping::new(
                    &group,
                    &annotate,
                    vec![SubtaskCategory::new("Fold", "Folds").unwrap()],
                    Vec::new(),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(payload.remove_node(DATASET_NODE_ID).is_err());
        payload.remove_node(&annotate).unwrap();
        assert_eq!(payload.nodes().len(), 1);
        assert!(payload.edges().is_empty());
        assert!(payload.subtasks_for(&annotate).next().is_none());
        assert!(payload.grouping_for(&group).is_none());
        assert_eq!(payload.validate(), Ok(()));
    }

    #[test]
    fn encoding_round_trips_and_rejects_other_versions_and_broken_graphs() {
        let (mut payload, annotate) = annotated_graph();
        let search = payload
            .add_child(
                &annotate,
                ExplorationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
            )
            .unwrap();
        if let ExplorationNodeKind::FindSimilarSubtasks(config) =
            payload.node_mut(&search).unwrap().kind_mut()
        {
            config.set_query("make the first fold");
        }
        let encoded = payload.encode().unwrap();
        let json: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(json["nodes"][1]["kind"]["type"], "break_down_subtasks");
        assert_eq!(json["nodes"][1]["kind"]["model"], DEFAULT_ANNOTATION_MODEL);
        assert_eq!(json["nodes"][2]["kind"]["query"], "make the first fold");
        assert_eq!(
            ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, &encoded).unwrap(),
            payload
        );
        assert!(ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION + 1, &encoded).is_err());
        assert!(
            ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, r#"{"nodes":[]}"#).is_err()
        );
        let two_parents = r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}},{"id":"a","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}},{"id":"b","kind":{"type":"break_down_subtasks","model":"m","sample_interval_seconds":0.5}}],"edges":[{"from":"dataset","to":"a"},{"from":"dataset","to":"b"},{"from":"a","to":"b"}]}"#;
        assert!(ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, two_parents).is_err());
        let unknown_field =
            r#"{"nodes":[{"id":"dataset","kind":{"type":"dataset"}}],"colour":"blue"}"#;
        assert!(ExplorationPayload::decode(EXPLORATION_ENCODING_VERSION, unknown_field).is_err());
    }
}
