//! The views saved curations publish, as another activity lists, reads and
//! unpublishes them: each one's name, the curation and node it comes from,
//! the dataset its rows belong to, what its columns hold, the statement it is
//! read through once it is published, and, while its models are still
//! answering, how far the run publishing it has got.
//!
//! Studio keeps a published view as a bare statement, which names neither
//! its curation nor the dataset its episodes are in; the curation that
//! published it records both, and its compiler says what each column of the
//! view is. The run publishing it is the backend's, which keeps it until the
//! view is published again.
use super::client::{CurationClient, FillRuns};
use super::document::{CurationGraph, CurationNode, CurationNodeKind};
use super::graph::PublishedViews;
use super::path::{ColumnMeaning, CurationPath};
use super::state::CurationDraft as _;
use crate::activities::episode::EpisodeListing;
use crate::client::queries::{QueryDomain, QueryRefresh};
use futures_util::future::try_join_all;
use spiral_robotics_common::curation::{
    CurationDocument, FillState, FillStatus, FillStep, PublishedView,
};
use spiral_robotics_common::datasets::DatasetSelection;
use std::collections::BTreeSet;

/// A column of a view that names or measures the subtasks its kept ranges
/// lie in, as Data shows them beside each range.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SubtaskColumn {
    /// A Break down's subtasks, which name a range by their description.
    Described(String),
    /// A Find similar's similarity of every subtask to its query.
    Similar(String),
    /// The subtasks a Group similar put in one category, and its name.
    Category { column: String, name: String },
}

impl SubtaskColumn {
    /// The view's column it reads.
    pub(crate) fn column(&self) -> &str {
        match self {
            Self::Described(column) | Self::Similar(column) => column,
            Self::Category { column, .. } => column,
        }
    }
    /// The columns of `node`'s table that name or measure subtasks.
    fn of(graph: &CurationGraph, node: &str) -> Vec<Self> {
        let Ok(table) = CurationPath::to(graph, node).and_then(|path| path.table()) else {
            return Vec::new();
        };
        table
            .columns()
            .iter()
            .filter_map(|column| {
                let name = column.name().to_owned();
                Some(match column.meaning() {
                    ColumnMeaning::Subtasks => Self::Described(name),
                    ColumnMeaning::Similarities => Self::Similar(name),
                    ColumnMeaning::Category(id) => {
                        let grouping = match graph.node(column.node()).map(CurationNode::kind) {
                            Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => grouping,
                            _ => return None,
                        };
                        Self::Category {
                            column: name,
                            name: grouping.category(id)?.name().to_owned(),
                        }
                    }
                    ColumnMeaning::Fact(_)
                    | ColumnMeaning::TrackedObjects
                    | ColumnMeaning::ProgressRates => return None,
                })
            })
            .collect()
    }
}

/// The curation a view comes from: its id and name, what the node it
/// publishes is, and whether the view is still what its saved recipe
/// compiles to.
#[derive(Clone, Debug, PartialEq)]
struct ViewRecipe {
    curation_id: String,
    curation: String,
    output: &'static str,
    current: bool,
}

/// A view a saved curation published.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CuratedView {
    view: String,
    recipe: ViewRecipe,
    dataset: EpisodeListing,
    subtasks: Vec<SubtaskColumn>,
    published: Option<String>,
    fill: Option<ViewFill>,
}

impl CuratedView {
    /// Every view the saved curations record publishing, by name. A
    /// curation saved in an encoding this Studio no longer opens is left out:
    /// it names no dataset this Studio can read.
    pub(crate) async fn all() -> Result<Vec<Self>, String> {
        let documents = CurationClient::current();
        let summaries = documents.list().await.map_err(|error| error.to_string())?;
        let stored = try_join_all(summaries.iter().map(|summary| documents.load(summary.id())))
            .await
            .map_err(|error| error.to_string())?;
        let published = documents
            .published_views()
            .await
            .map_err(|error| error.to_string())?;
        let mut views = stored
            .iter()
            .flat_map(Self::published_by)
            .map(|view| view.reading_from(&published))
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.view.cmp(&right.view));
        Ok(views)
    }

    /// The view with the statement `published` keeps under its name, which
    /// as a SQL identifier ignores case.
    fn reading_from(self, published: &[PublishedView]) -> Self {
        let statement = published
            .iter()
            .find(|stored| stored.name().eq_ignore_ascii_case(&self.view))
            .map(|stored| stored.statement().to_owned());
        Self {
            published: statement,
            ..self
        }
    }

    /// Drop the view and forget it in the curation that published it, saved
    /// at the revision just read, so a save landing in between is refused
    /// rather than lost. The curation, open in Curation, takes the change
    /// when the catalog is read again.
    pub(crate) async fn unpublish(&self) -> Result<(), String> {
        PublishedViews::drop_view(self.view.clone()).await?;
        let documents = CurationClient::current();
        let stored = documents
            .load(&self.recipe.curation_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut graph = CurationGraph::decode(stored.encoding(), stored.payload())?;
        let node = graph
            .publications()
            .find(|(published, _)| published.view() == self.view)
            .map(|(published, _)| published.node().to_owned());
        if let Some(node) = node {
            graph.remove_publication(&node)?;
            documents
                .save(&stored.carrying(&graph))
                .await
                .map_err(|error| error.to_string())?;
        }
        QueryRefresh::current().invalidate(&[QueryDomain::Catalog]);
        Ok(())
    }

    /// The views one stored curation publishes.
    fn published_by(document: &CurationDocument) -> Vec<Self> {
        let Ok(graph) = CurationGraph::decode(document.encoding(), document.payload()) else {
            return Vec::new();
        };
        let Some(dataset) = graph.listing() else {
            return Vec::new();
        };
        graph
            .publications()
            .map(|(published, node)| Self {
                view: published.view().to_owned(),
                recipe: ViewRecipe {
                    curation_id: document.id().to_owned(),
                    curation: document.name().to_owned(),
                    output: node.kind().label(),
                    current: published.is_current(&graph),
                },
                dataset: dataset.clone(),
                subtasks: SubtaskColumn::of(&graph, node.id()),
                published: None,
                fill: None,
            })
            .collect()
    }

    /// The name it is listed and read by.
    pub(crate) fn view(&self) -> &str {
        &self.view
    }
    /// The statement it is read through, once its run published it.
    pub(crate) fn statement(&self) -> Option<&str> {
        self.published.as_deref()
    }
    /// The curation that published it.
    pub(crate) fn curation(&self) -> &str {
        &self.recipe.curation
    }
    /// What the node it publishes is, such as Select.
    pub(crate) fn output(&self) -> &'static str {
        self.recipe.output
    }
    /// The dataset its episodes belong to.
    pub(crate) fn dataset(&self) -> &DatasetSelection {
        self.dataset.dataset()
    }
    /// How that dataset lists its episodes.
    pub(crate) fn listing(&self) -> &EpisodeListing {
        &self.dataset
    }
    /// Whether it is still what its curation's saved recipe compiles to.
    pub(crate) fn is_current(&self) -> bool {
        self.recipe.current
    }
    /// How the run publishing it stands, while it runs or once it failed.
    pub(crate) fn fill(&self) -> Option<&ViewFill> {
        self.fill.as_ref()
    }
    /// Its columns that name or measure the subtasks its ranges lie in.
    pub(crate) fn subtasks(&self) -> &[SubtaskColumn] {
        &self.subtasks
    }
}

/// Where the run publishing a view stands, while it runs or once it failed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewFill {
    Running(FillStep),
    Failed(String),
}

impl ViewFill {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
    /// What the run is doing, in the words Data shows: the node whose
    /// answers it stores and how many of the episodes that need them are
    /// answered; or why it stopped.
    pub(crate) fn doing(&self) -> String {
        match self {
            Self::Running(step) => match step.total() {
                Some(total) => format!("{}: {} of {total} episodes", step.node(), step.done()),
                None => format!("{}: finding the episodes to ask", step.node()),
            },
            Self::Failed(reason) => reason.clone(),
        }
    }
}

/// The runs publishing curated views, as Data polls them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CuratedFills(Vec<FillStatus>);

impl CuratedFills {
    pub(crate) async fn read() -> Result<Self, String> {
        FillRuns::current()
            .publications()
            .await
            .map(Self)
            .map_err(|error| error.to_string())
    }
    /// The views a run is filling now.
    pub(crate) fn running(&self) -> BTreeSet<String> {
        self.0
            .iter()
            .filter(|status| !status.state().is_settled())
            .flat_map(|status| status.views().iter().cloned())
            .collect()
    }
    /// `views`, each with how the run publishing it stands.
    pub(crate) fn applied(&self, views: &[CuratedView]) -> Vec<CuratedView> {
        views
            .iter()
            .map(|view| CuratedView {
                fill: self.of(&view.view),
                ..view.clone()
            })
            .collect()
    }
    /// How the run publishing `view` stands, unless it finished.
    fn of(&self, view: &str) -> Option<ViewFill> {
        let status = self
            .0
            .iter()
            .find(|status| status.views().iter().any(|published| published == view))?;
        match status.state() {
            FillState::Running(step) => Some(ViewFill::Running(step.clone())),
            FillState::Failed(reason) => Some(ViewFill::Failed(reason.clone())),
            FillState::Stopped => Some(ViewFill::Failed(
                "Its run was stopped before it finished.".into(),
            )),
            FillState::Finished => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::curation::EpisodeFailure;

    fn view(name: &str) -> CuratedView {
        CuratedView {
            view: name.to_owned(),
            recipe: ViewRecipe {
                curation_id: "curation-1".to_owned(),
                curation: "Cloth folding".to_owned(),
                output: "Score progress",
                current: true,
            },
            dataset: EpisodeListing::mcap_fixture("dataset_folding"),
            subtasks: Vec::new(),
            published: None,
            fill: None,
        }
    }

    fn status(views: &[&str], state: FillState) -> FillStatus {
        FillStatus::new(
            "fill-1".to_owned(),
            views.iter().map(|view| (*view).to_owned()).collect(),
            state,
            Vec::<EpisodeFailure>::new(),
        )
    }

    #[test]
    fn a_view_carries_how_the_run_publishing_it_stands_until_it_finishes() {
        let fills = CuratedFills(vec![
            status(
                &["moving", "folds"],
                FillState::Running(FillStep::new("Score progress", 12, Some(340))),
            ),
            status(
                &["grasps"],
                FillState::Failed("the token was refused".into()),
            ),
            status(&["reaches"], FillState::Finished),
        ]);
        assert_eq!(
            fills.running(),
            BTreeSet::from(["folds".to_owned(), "moving".to_owned()])
        );
        let views = fills.applied(&[
            view("folds"),
            view("grasps"),
            view("reaches"),
            view("unfilled"),
        ]);
        let folds = views[0].fill().unwrap();
        assert!(folds.is_running());
        assert_eq!(folds.doing(), "Score progress: 12 of 340 episodes");
        assert_eq!(
            views[1].fill(),
            Some(&ViewFill::Failed("the token was refused".into()))
        );
        assert_eq!((views[2].fill(), views[3].fill()), (None, None));
        let finding = ViewFill::Running(FillStep::new("Break down subtasks", 0, None));
        assert_eq!(
            finding.doing(),
            "Break down subtasks: finding the episodes to ask"
        );
    }
}
