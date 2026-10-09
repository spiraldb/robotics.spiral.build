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
    /// The resources the last successful read returned.
    listed: Signal<Vec<ProjectResource>>,
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
            || DatasetClient::current().project_resources(),
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
        self.listed
            .read()
            .iter()
            .filter_map(ProjectResource::dataset)
            .collect()
    }

    /// The names of the datasets a recording may join: Studio's MCAP
    /// datasets, each made from its own table of files. A LeRobot dataset,
    /// made from a Location over its repository, is not one.
    pub(crate) fn recording_destinations(self) -> Vec<String> {
        RecordingDestinations(&self.listed.read()).names()
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

/// The datasets of one catalog that new MCAP recordings may join.
struct RecordingDestinations<'a>(&'a [ProjectResource]);

impl RecordingDestinations<'_> {
    fn names(&self) -> Vec<String> {
        self.0
            .iter()
            .filter(|resource| resource.dataset().is_some())
            .filter_map(|resource| {
                let label = resource.selection().label();
                let name = label.strip_prefix("dataset_")?.to_owned();
                let files = format!("dataset_files_{name}");
                self.0
                    .iter()
                    .any(|other| other.selection().label() == files && other.kind() == "table")
                    .then_some(name)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::DatasetSelection;

    fn resource(name: &str, kind: &str) -> ProjectResource {
        ProjectResource::new(
            DatasetSelection::new(vec![name.into()]).unwrap(),
            kind.into(),
            (name.into(), "v".into()),
        )
    }

    #[test]
    fn a_recording_joins_an_mcap_dataset_and_never_a_lerobot_one() {
        let catalog = [
            resource("dataset_bench", "dataset"),
            resource("dataset_files_bench", "table"),
            resource("dataset_so101", "dataset"),
            resource("dataset_files_so101", "location"),
            resource("dataset_hosted", "dataset"),
        ];
        assert_eq!(RecordingDestinations(&catalog).names(), ["bench"]);
    }
}
