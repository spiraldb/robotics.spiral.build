//! The curation payload: the graph, per-node configuration, the outputs
//! published as catalog views, and what the measurements Spiral cannot run yet
//! produced, encoded as JSON text in `CurationDocument::payload`. What a
//! compiled node reads is a preview held for the session only (see
//! `preview`). Node positions are not stored; the canvas lays the graph out
//! from its edges every time.
//!
//! The graph's grammar lives here too. Every node hands on a population of
//! one kind — episodes, subtasks, or moments (see `population`) — and what
//! may follow a node is decided by that kind and by the node itself.
//!
//! This is the contract every Curation view shares. Add fields and helpers;
//! never rename or remove what is here without checking the other views.
mod publication;
mod selection;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use publication::PublishedOutput;
pub(crate) use selection::{Comparison, EpisodeFact, Selection, SelectionBasis};

/// Encoding 2 keeps a recipe only for the nodes that compile to Spiral SQL.
/// Encoding 1 stored what they measured and is not converted.
pub(crate) const CURATION_ENCODING_VERSION: u32 = 2;
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
    subtasks: Vec<SubtaskRow>,
    #[serde(default)]
    groupings: Vec<SubtaskGrouping>,
    #[serde(default)]
    rankings: Vec<SubtaskRanking>,
    #[serde(default)]
    published: Vec<PublishedOutput>,
}

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

/// What a node is, with the configuration its kind needs. The dataset hands
/// on its episodes, a measurement hands on what it measured over its input,
/// and a Select hands on the part of its input its rule keeps.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CurationNodeKind {
    Dataset,
    BreakDownSubtasks(SubtaskAnnotation),
    FindSimilarSubtasks(SubtaskSearch),
    GroupSimilarSubtasks,
    IdentifyUnproductiveMoments(MomentDetection),
    Select(Selection),
}
impl CurationNodeKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Dataset => "dataset",
            Self::BreakDownSubtasks(_) => "break_down_subtasks",
            Self::FindSimilarSubtasks(_) => "find_similar_subtasks",
            Self::GroupSimilarSubtasks => "group_similar_subtasks",
            Self::IdentifyUnproductiveMoments(_) => "identify_unproductive_moments",
            Self::Select(_) => "select",
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Dataset => "Dataset",
            Self::BreakDownSubtasks(_) => "Break down subtasks",
            Self::FindSimilarSubtasks(_) => "Find similar subtasks",
            Self::GroupSimilarSubtasks => "Group similar subtasks",
            Self::IdentifyUnproductiveMoments(_) => "Identify unproductive moments",
            Self::Select(_) => "Select",
        }
    }
    /// What a Select after this node keeps by; `None` for a node whose output
    /// has nothing to select by, and for a Select, which defers to its parent.
    fn selection_basis(&self) -> Option<SelectionBasis> {
        match self {
            Self::Dataset => Some(SelectionBasis::EpisodeFacts),
            Self::FindSimilarSubtasks(_) => Some(SelectionBasis::Similarity),
            Self::GroupSimilarSubtasks => Some(SelectionBasis::Categories),
            Self::IdentifyUnproductiveMoments(_) => Some(SelectionBasis::Progress),
            Self::BreakDownSubtasks(_) | Self::Select(_) => None,
        }
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::BreakDownSubtasks(annotation) => annotation.validate(),
            Self::Select(selection) => selection.validate(),
            Self::Dataset
            | Self::FindSimilarSubtasks(_)
            | Self::GroupSimilarSubtasks
            | Self::IdentifyUnproductiveMoments(_) => Ok(()),
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

impl EpisodeLedger for SubtaskAnnotation {
    fn completed_mut(&mut self) -> &mut Vec<EpisodeId> {
        &mut self.completed_episodes
    }
    fn failed_mut(&mut self) -> &mut Vec<EpisodeFailure> {
        &mut self.failed_episodes
    }
}

/// A measurement that reads its input one episode at a time, and so keeps
/// which episodes it has read and which it could not. A later success
/// replaces an earlier failure.
trait EpisodeLedger {
    fn completed_mut(&mut self) -> &mut Vec<EpisodeId>;
    fn failed_mut(&mut self) -> &mut Vec<EpisodeFailure>;
    fn mark_completed(&mut self, episode: &EpisodeId) {
        if !self.completed_mut().contains(episode) {
            self.completed_mut().push(episode.clone());
        }
        self.failed_mut()
            .retain(|failure| &failure.episode != episode);
    }
    fn mark_failed(&mut self, episode: &EpisodeId, reason: impl Into<String>) {
        self.failed_mut()
            .retain(|failure| &failure.episode != episode);
        self.failed_mut().push(EpisodeFailure {
            episode: episode.clone(),
            reason: reason.into(),
        });
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

/// One Identify unproductive moments node. Its scorer is pinned, so there is
/// nothing to configure; what it reads is the preview's, never the document's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MomentDetection {}

/// The last query of a Find similar subtasks node. Its ranking is stored
/// beside it as a `SubtaskRanking`, which names the query it answers.
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
impl SubtaskKey {
    fn is(&self, row: &SubtaskRow) -> bool {
        row.is(&self.episode, self.start_time)
    }
}

/// What one Find similar node found: the query it answered and jev's
/// probability for each row it scored. A row without a score was not answered.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubtaskRanking {
    node: String,
    query: String,
    scores: Vec<SubtaskScore>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SubtaskScore {
    row: SubtaskKey,
    probability: f64,
}

impl SubtaskRanking {
    pub(crate) fn new(
        node: impl Into<String>,
        query: impl Into<String>,
        scores: impl IntoIterator<Item = (SubtaskKey, f64)>,
    ) -> Result<Self, String> {
        let ranking = Self {
            node: node.into(),
            query: query.into(),
            scores: scores
                .into_iter()
                .map(|(row, probability)| SubtaskScore { row, probability })
                .collect(),
        };
        if ranking
            .scores
            .iter()
            .any(|score| !(0.0..=1.0).contains(&score.probability))
        {
            return Err("A similarity must be between 0 and 1.".into());
        }
        Ok(ranking)
    }
    pub(crate) fn query(&self) -> &str {
        &self.query
    }
    /// jev's probability for one row, when it answered for it.
    pub(crate) fn probability(&self, row: &SubtaskRow) -> Option<f64> {
        self.scores
            .iter()
            .find(|score| score.row.is(row))
            .map(|score| score.probability)
    }
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
    /// 5.5 kB: 311 of them are 1.7 MB, five per cent of what one curation
    /// may carry (`MAX_CURATION_PAYLOAD_BYTES`).
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
    /// The category one row joined, when this grouping placed it.
    pub(crate) fn category_of(&self, row: &SubtaskRow) -> Option<&str> {
        self.assignments
            .iter()
            .find(|assignment| row.is(&assignment.episode, assignment.start_time))
            .map(|assignment| assignment.category.as_str())
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

impl CurationGraph {
    /// A graph with only its dataset node.
    pub(crate) fn new(dataset: Option<DatasetSelection>) -> Self {
        Self {
            dataset,
            nodes: vec![CurationNode {
                id: DATASET_NODE_ID.to_owned(),
                kind: CurationNodeKind::Dataset,
            }],
            edges: Vec::new(),
            subtasks: Vec::new(),
            groupings: Vec::new(),
            rankings: Vec::new(),
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
            | CurationNodeKind::GroupSimilarSubtasks => Some(PopulationKind::Subtasks),
            CurationNodeKind::IdentifyUnproductiveMoments(_) => Some(PopulationKind::Moments),
            CurationNodeKind::Select(_) => self.population_kind(self.parent_of(node)?),
        }
    }
    /// What a Select placed after `node` would keep by. Episode filters chain,
    /// so a Select of episodes may follow another; a second Select of the same
    /// probabilities or categories would only repeat the first.
    fn selection_basis_after(&self, node: &str) -> Option<SelectionBasis> {
        match self.node(node)?.kind() {
            CurationNodeKind::Select(selection) => {
                (selection.basis() == SelectionBasis::EpisodeFacts).then_some(selection.basis())
            }
            kind => kind.selection_basis(),
        }
    }
    /// What may follow `node`, in menu order, each with its default
    /// configuration: the measurements that read what it hands on, then a
    /// Select when there is something to select by.
    pub(crate) fn child_kinds(&self, node: &str) -> Vec<CurationNodeKind> {
        let Some(parent) = self.node(node) else {
            return Vec::new();
        };
        let mut kinds = match (self.population_kind(node), parent.kind()) {
            (Some(PopulationKind::Episodes), _) => vec![
                CurationNodeKind::BreakDownSubtasks(SubtaskAnnotation::default()),
                CurationNodeKind::IdentifyUnproductiveMoments(MomentDetection::default()),
            ],
            // Break down what is left once the unproductive moments are out.
            (Some(PopulationKind::Moments), _) => vec![CurationNodeKind::BreakDownSubtasks(
                SubtaskAnnotation::default(),
            )],
            (
                Some(PopulationKind::Subtasks),
                CurationNodeKind::BreakDownSubtasks(_) | CurationNodeKind::Select(_),
            ) => vec![
                CurationNodeKind::FindSimilarSubtasks(SubtaskSearch::default()),
                CurationNodeKind::GroupSimilarSubtasks,
            ],
            _ => Vec::new(),
        };
        if let Some(basis) = self.selection_basis_after(node) {
            kinds.push(CurationNodeKind::Select(Selection::for_basis(basis)));
        }
        kinds
    }
    fn accepts_child(&self, parent: &str, child: &CurationNodeKind) -> bool {
        match child {
            CurationNodeKind::Select(selection) => {
                self.selection_basis_after(parent) == Some(selection.basis())
            }
            child => self
                .child_kinds(parent)
                .iter()
                .any(|kind| kind.key() == child.key()),
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
                | CurationNodeKind::GroupSimilarSubtasks
                | CurationNodeKind::Select(_) => current = self.parent_of(current)?,
                _ => return None,
            }
        }
        None
    }
    /// The node a Select's rule is stated in terms of: the nearest one above
    /// it that is not itself a Select.
    pub(crate) fn selection_measure(&self, select: &str) -> Option<&CurationNode> {
        let mut current = self.parent_of(select)?;
        for _ in 0..self.nodes.len() {
            let node = self.node(current)?;
            match node.kind() {
                CurationNodeKind::Select(_) => current = self.parent_of(current)?,
                _ => return Some(node),
            }
        }
        None
    }
    /// The curation's outputs: every Select nothing follows. Each is one
    /// selection of the data to train on.
    pub(crate) fn outputs(&self) -> Vec<&CurationNode> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.kind(), CurationNodeKind::Select(_)))
            .filter(|node| self.children_of(node.id()).is_empty())
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
    /// Remove a node, its descendants, and everything they produced. A branch
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
        self.subtasks.retain(|row| !doomed.contains(&row.node));
        self.groupings
            .retain(|grouping| !doomed.contains(&grouping.node));
        self.rankings
            .retain(|ranking| !doomed.contains(&ranking.node));
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
    /// Record a failed attempt at one episode of a Break down subtasks node;
    /// what it stored for the episode before stays.
    pub(crate) fn record_episode_failure(
        &mut self,
        node: &str,
        episode: &EpisodeId,
        reason: impl Into<String>,
    ) -> Result<(), String> {
        self.annotation_mut(node)?.mark_failed(episode, reason);
        Ok(())
    }
    /// What a Find similar node last found, whatever it was asked.
    pub(crate) fn ranking_for(&self, node: &str) -> Option<&SubtaskRanking> {
        self.rankings.iter().find(|ranking| ranking.node == node)
    }
    /// A search's stored ranking while it still answers the search's query. A
    /// ranking of an earlier question says nothing about this one.
    pub(crate) fn current_ranking(&self, node: &str) -> Option<&SubtaskRanking> {
        let CurationNodeKind::FindSimilarSubtasks(search) = self.node(node)?.kind() else {
            return None;
        };
        self.ranking_for(node)
            .filter(|ranking| ranking.query().trim() == search.query().trim())
    }
    /// Store a Find similar node's ranking, replacing any earlier one.
    pub(crate) fn set_ranking(&mut self, ranking: SubtaskRanking) -> Result<(), String> {
        match self.node(&ranking.node).map(CurationNode::kind) {
            Some(CurationNodeKind::FindSimilarSubtasks(_)) => {}
            Some(_) => return Err("That node does not search subtasks.".into()),
            None => return Err("Unknown node.".into()),
        }
        self.rankings
            .retain(|existing| existing.node != ranking.node);
        self.rankings.push(ranking);
        Ok(())
    }
    fn annotation_mut(&mut self, node: &str) -> Result<&mut SubtaskAnnotation, String> {
        match self.node_mut(node).map(|node| &mut node.kind) {
            Some(CurationNodeKind::BreakDownSubtasks(annotation)) => Ok(annotation),
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
        match self.node(&grouping.node).map(CurationNode::kind) {
            Some(CurationNodeKind::GroupSimilarSubtasks) => {}
            Some(_) => return Err("That node does not group subtasks.".into()),
            None => return Err("Unknown node.".into()),
        }
        if self.subtask_source(&grouping.node) != Some(grouping.source.as_str()) {
            return Err("A grouping's source must be the annotation its rows come from.".into());
        }
        self.groupings
            .retain(|existing| existing.node != grouping.node);
        self.groupings.push(grouping);
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.validate_nodes()?;
        self.validate_edges()?;
        self.validate_subtasks()?;
        self.validate_stored_outputs()?;
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
    /// through Selects, which only terminates on a graph without cycles.
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
        Ok(())
    }
    fn validate_subtasks(&self) -> Result<(), String> {
        let mut by_episode: BTreeMap<(&str, &EpisodeId), Vec<&SubtaskRow>> = BTreeMap::new();
        for row in &self.subtasks {
            row.validate()?;
            match self.node(&row.node).map(CurationNode::kind) {
                Some(CurationNodeKind::BreakDownSubtasks(_)) => {}
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
        Ok(())
    }
    /// What the searches and groupings stored belongs to a node of their own
    /// kind, once per node.
    fn validate_stored_outputs(&self) -> Result<(), String> {
        let mut grouped = BTreeSet::new();
        for grouping in &self.groupings {
            grouping.validate()?;
            if !grouped.insert(grouping.node.as_str()) {
                return Err(format!("Node “{}” has two groupings.", grouping.node));
            }
            match self.node(&grouping.node).map(CurationNode::kind) {
                Some(CurationNodeKind::GroupSimilarSubtasks) => {}
                _ => {
                    return Err(format!(
                        "Grouping refers to unknown node “{}”.",
                        grouping.node
                    ));
                }
            }
            if self.subtask_source(&grouping.node) != Some(grouping.source.as_str()) {
                return Err(format!(
                    "Grouping “{}” does not read the annotation above it.",
                    grouping.node
                ));
            }
        }
        let mut ranked = BTreeSet::new();
        for ranking in &self.rankings {
            if !ranked.insert(ranking.node.as_str()) {
                return Err(format!("Node “{}” has two rankings.", ranking.node));
            }
            if !matches!(
                self.node(&ranking.node).map(CurationNode::kind),
                Some(CurationNodeKind::FindSimilarSubtasks(_))
            ) {
                return Err(format!(
                    "Ranking refers to unknown node “{}”.",
                    ranking.node
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
