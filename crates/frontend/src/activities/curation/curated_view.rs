//! The views saved curations publish, as another activity lists and
//! unpublishes them: each one's catalog name, the curation and output it
//! comes from, and the dataset its rows belong to.
//!
//! The catalog holds a view as a bare statement, which names neither its
//! curation nor the dataset its episodes are in; the curation that published
//! it records both.
use super::annotate::SubtaskRow;
use super::client::CurationClient;
use super::document::CurationGraph;
use super::graph::PublishedViews;
use super::score_progress::{Moment, ProgressRate};
use super::state::CurationDraft as _;
use crate::client::queries::{QueryDomain, QueryRefresh};
use futures_util::future::try_join_all;
use spiral_robotics_common::curation::CurationDocument;
use spiral_robotics_common::datasets::DatasetSelection;

/// A catalog view a saved curation published.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CuratedView {
    view: String,
    curation_id: String,
    curation: String,
    output: &'static str,
    dataset: DatasetSelection,
    current: bool,
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
        let mut views = stored
            .iter()
            .flat_map(Self::published_by)
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.view.cmp(&right.view));
        Ok(views)
    }

    /// Drop the view from the catalog and forget it in the curation that
    /// published it, saved at the revision just read, so a save landing in
    /// between is refused rather than lost. The curation, open in Curation,
    /// takes the change when the catalog is read again.
    pub(crate) async fn unpublish(&self) -> Result<(), String> {
        PublishedViews::drop_view(self.view.clone()).await?;
        let documents = CurationClient::current();
        let stored = documents
            .load(&self.curation_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut graph = CurationGraph::decode(stored.encoding(), stored.payload())?;
        let node = graph
            .publications()
            .find(|(published, _)| published.view() == self.view)
            .map(|(published, _)| published.node().to_owned());
        if let Some(node) = node {
            graph.remove_publication(&node, stored.revision())?;
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
        let Some(dataset) = graph.dataset() else {
            return Vec::new();
        };
        graph
            .publications()
            .map(|(published, node)| Self {
                view: published.view().to_owned(),
                curation_id: document.id().to_owned(),
                curation: document.name().to_owned(),
                output: node.kind().label(),
                dataset: dataset.clone(),
                current: published.is_current(document.revision()),
            })
            .collect()
    }

    /// The name the catalog reads it by.
    pub(crate) fn view(&self) -> &str {
        &self.view
    }
    /// The curation that published it.
    pub(crate) fn curation(&self) -> &str {
        &self.curation
    }
    /// What the node it publishes is, such as Select.
    pub(crate) fn output(&self) -> &'static str {
        self.output
    }
    /// The dataset its episodes belong to.
    pub(crate) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    /// Whether it is still what its curation's saved recipe compiles to.
    pub(crate) fn is_current(&self) -> bool {
        self.current
    }
    /// A number one of its columns holds, printed the way Curation prints
    /// it, when a node publishes the column: a progress rate reads "0.69×",
    /// a similarity "69%".
    pub(crate) fn reading(column: &str, value: f64) -> Option<String> {
        match column {
            Moment::PROGRESS_RATE => Some(ProgressRate::label(value)),
            column if SubtaskRow::is_similarity(column) => {
                Some(SubtaskRow::similarity_label(value))
            }
            _ => None,
        }
    }
}
