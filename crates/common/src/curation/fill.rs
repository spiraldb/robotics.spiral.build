//! A fill run: the backend asks each model about the episodes that need its
//! answers, writes them as columns of the dataset's answers table, and then
//! publishes the views that read those columns.
//!
//! The browser compiles the run: for each node that asks a model, in path
//! order, the column its answers go in, which episodes reach the node, and
//! how one episode's answer is computed. The backend skips every episode the
//! column already holds, so a run cut short resumes when it is asked again.
//! Publishing is one run over the whole dataset; a Score progress preview is
//! one run over the episodes it previews, publishing nothing.
use super::PublishedView;
use super::answers::AnswerColumn;
use super::subtasks::{SubtaskBreakdown, SubtaskClassification, SubtaskSimilarity};
use crate::datasets::{DatasetSelection, EpisodeId};
use serde::{Deserialize, Serialize};

/// How one column's answer is computed for one episode.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnswerSource {
    /// A statement over the whole dataset, answering `episode_id` and
    /// `answer` in one row per episode it measures: one read answers every
    /// episode the column is filled for, where reading each episode alone
    /// would read the dataset's tables once per episode.
    DatasetStatement { statement: String },
    /// The progress service's scores of the episode's video on `camera`,
    /// from the checkpoint `model`, as the service `revision` computes them.
    ProgressService {
        camera: String,
        model: String,
        revision: String,
    },
    /// The object tracking service's tracks of the objects `prompt` names in
    /// the episode's video on `camera`, sampled `rate` pictures a second,
    /// from the checkpoint `model`, as the service `revision` computes them.
    ObjectTracking {
        camera: String,
        prompt: String,
        rate: u32,
        model: String,
        revision: String,
    },
    /// Break down's subtasks of the episode's video, asked of a multimodal
    /// model through OpenRouter.
    BreakDown(SubtaskBreakdown),
    /// Find similar's similarity of each subtask a Break down stored for the
    /// episode, asked of a decision model through OpenRouter.
    Similarity(SubtaskSimilarity),
    /// Group similar's category of each subtask a Break down stored for the
    /// episode, asked of a decision model through OpenRouter.
    Classify(SubtaskClassification),
}

/// Which episodes a column is filled for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "episodes", rename_all = "snake_case")]
pub enum FillEpisodes {
    /// Those a read-only statement lists by `episode_id`: every episode that
    /// reaches the node.
    Reaching(String),
    /// These, as a preview names them.
    Listed(Vec<EpisodeId>),
}

/// One column a run writes: the node that asks for it, as the run reports
/// it, the column, the episodes it is filled for, and how each answer is
/// computed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnFill {
    node: String,
    column: AnswerColumn,
    episodes: FillEpisodes,
    source: AnswerSource,
}

impl ColumnFill {
    pub fn new(
        node: impl Into<String>,
        column: AnswerColumn,
        episodes: FillEpisodes,
        source: AnswerSource,
    ) -> Self {
        Self {
            node: node.into(),
            column,
            episodes,
            source,
        }
    }
    pub fn node(&self) -> &str {
        &self.node
    }
    pub fn column(&self) -> &AnswerColumn {
        &self.column
    }
    pub fn episodes(&self) -> &FillEpisodes {
        &self.episodes
    }
    pub fn source(&self) -> &AnswerSource {
        &self.source
    }
}

/// A fill run: the dataset whose answers table it writes, every column in
/// the order they are filled, and the views created or replaced once all of
/// them are.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FillRequest {
    dataset: DatasetSelection,
    columns: Vec<ColumnFill>,
    #[serde(default)]
    views: Vec<PublishedView>,
}

impl FillRequest {
    pub fn new(dataset: DatasetSelection, columns: Vec<ColumnFill>) -> Self {
        Self {
            dataset,
            columns,
            views: Vec::new(),
        }
    }
    /// The same run, publishing `views` once every column is filled.
    pub fn publishing(mut self, views: Vec<PublishedView>) -> Self {
        self.views = views;
        self
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn columns(&self) -> &[ColumnFill] {
        &self.columns
    }
    pub fn views(&self) -> &[PublishedView] {
        &self.views
    }
}

/// What a running fill is doing: filling the column `node` asks for, `done`
/// of the `total` episodes that need it, or still finding them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FillStep {
    node: String,
    done: usize,
    total: Option<usize>,
}

impl FillStep {
    pub fn new(node: impl Into<String>, done: usize, total: Option<usize>) -> Self {
        Self {
            node: node.into(),
            done,
            total,
        }
    }
    pub fn node(&self) -> &str {
        &self.node
    }
    pub fn done(&self) -> usize {
        self.done
    }
    pub fn total(&self) -> Option<usize> {
        self.total
    }
}

/// Where a fill run stands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum FillState {
    Running(FillStep),
    /// Every column is filled and every view published.
    Finished,
    /// Stopped before it finished; what it had answered is written.
    Stopped,
    /// It could not go on, for this reason; what it had answered is written.
    Failed(String),
}

impl FillState {
    pub fn is_settled(&self) -> bool {
        !matches!(self, Self::Running(_))
    }
}

/// An episode the run asked about whose answer failed, and why.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeFailure {
    episode: EpisodeId,
    reason: String,
}

impl EpisodeFailure {
    pub fn new(episode: EpisodeId, reason: impl Into<String>) -> Self {
        Self {
            episode,
            reason: reason.into(),
        }
    }
    pub fn episode(&self) -> &EpisodeId {
        &self.episode
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Where one fill run stands, as each poll reads it: the views it publishes,
/// its state, and the episodes whose answers failed so far.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FillStatus {
    id: String,
    views: Vec<String>,
    state: FillState,
    failures: Vec<EpisodeFailure>,
}

impl FillStatus {
    pub fn new(
        id: String,
        views: Vec<String>,
        state: FillState,
        failures: Vec<EpisodeFailure>,
    ) -> Self {
        Self {
            id,
            views,
            state,
            failures,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn views(&self) -> &[String] {
        &self.views
    }
    pub fn state(&self) -> &FillState {
        &self.state
    }
    pub fn failures(&self) -> &[EpisodeFailure] {
        &self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_travels_as_the_columns_it_fills_and_the_views_it_publishes() {
        let dataset = DatasetSelection::new("folding").unwrap();
        let column = AnswerColumn::of("similarity", &["q"]).unwrap();
        let request = FillRequest::new(
            dataset,
            vec![ColumnFill::new(
                "Find similar subtasks",
                column,
                FillEpisodes::Listed(vec![EpisodeId::try_from("0".repeat(32)).unwrap()]),
                AnswerSource::Similarity(SubtaskSimilarity::new(
                    AnswerColumn::of("breakdown_task", &["m"]).unwrap(),
                    "fold the towel",
                    "typesafe/jev-1.13",
                )),
            )],
        )
        .publishing(vec![PublishedView::new("folds", "SELECT 1")]);
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["columns"][0]["source"]["kind"], "similarity");
        assert_eq!(json["columns"][0]["source"]["query"], "fold the towel");
        assert_eq!(json["columns"][0]["episodes"]["kind"], "listed");
        assert_eq!(
            serde_json::from_value::<FillRequest>(json).unwrap(),
            request
        );
        let running = FillState::Running(FillStep::new("Score progress", 3, Some(10)));
        assert!(!running.is_settled() && FillState::Stopped.is_settled());
        assert_eq!(
            serde_json::to_value(FillState::Failed("no token".into())).unwrap(),
            serde_json::json!({"state": "failed", "detail": "no token"})
        );
    }
}
