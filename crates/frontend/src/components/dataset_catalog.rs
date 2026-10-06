//! The project's datasets, read once for every surface that asks for one.
//!
//! Curation's dataset node, and Training's and Rollout's session setup, all ask
//! the same question and all need the same three answers: the catalog is
//! still loading, the catalog could not be read, or here are the datasets.
//! [`DatasetCatalog`] owns the read, including keeping the last good rows on
//! screen through a failed refresh.

use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSummary, ProjectResource};

use crate::activities::data::DatasetClient;
use crate::client::CommandError;
use crate::client::queries::{QueryDomain, QueryPolicy, QueryRefresh};
use crate::shell::project::ProjectSelection;

/// The project's datasets as one surface sees them.
///
/// Read once per mounted field. A refresh that fails leaves the rows it had:
/// naming a dataset is how most of Studio's work starts, and a momentary
/// catalog failure should not empty the field the operator is typing into.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetCatalog {
    /// The datasets the last successful read returned.
    listed: Signal<Vec<DatasetSummary>>,
    /// Why the last read failed, if it did. Cleared by the next success.
    problem: Signal<Option<CommandError>>,
    reading: Signal<bool>,
}

impl DatasetCatalog {
    pub(crate) fn use_catalog() -> Self {
        let catalog = use_hook(|| Self {
            listed: Signal::new(Vec::new()),
            problem: Signal::new(None),
            reading: Signal::new(false),
        });
        let selection = ProjectSelection::current();
        let available = use_memo(move || selection.catalog_available());
        let request = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
            move || *available.read(),
            || {
                let resources = DatasetClient::current().project_resources();
                async move {
                    resources.await.map(|resources| {
                        resources
                            .iter()
                            .filter_map(ProjectResource::dataset)
                            .collect::<Vec<_>>()
                    })
                }
            },
        );
        let mut catalog = catalog;
        use_effect(move || {
            let response = request.read();
            if !request.finished() {
                catalog.reading.set(true);
                return;
            }
            catalog.reading.set(false);
            match response.as_ref() {
                Some(Ok(datasets)) => {
                    catalog.listed.set(datasets.clone());
                    catalog.problem.set(None);
                }
                Some(Err(error)) => catalog.problem.set(Some(error.clone())),
                None => {}
            }
        });
        catalog
    }

    pub(crate) fn datasets(self) -> Vec<DatasetSummary> {
        self.listed.read().clone()
    }

    pub(crate) fn reading(self) -> bool {
        *self.reading.read()
    }

    pub(crate) fn problem(self) -> Option<CommandError> {
        self.problem.read().clone()
    }

    /// Ask for the catalog again. Every field reading it gets the new answer.
    pub(crate) fn reread(self) {
        QueryRefresh::current().invalidate(&[QueryDomain::Catalog]);
    }
}
