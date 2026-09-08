//! Shared table schema discovery for Data and Workflow.
use dioxus::prelude::*;
/// Completed schema reads are local to the workspace, refreshed by catalog mutations.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RelationCatalogState {
    result: ReadSignal<Option<Result<spiral_robotics_common::relations::RelationCatalog, String>>>,
}
impl RelationCatalogState {
    pub(crate) fn use_catalog() -> Self {
        let sessions = crate::shell::project::ProjectSessionSignals::current();
        let available = use_memo(move || sessions.catalog_available());
        let request = crate::client::queries::QueryPolicy::on_demand(
            crate::client::queries::QueryDomain::Catalog,
        )
        .use_when(
            move || *available.read(),
            || async {
                super::DatasetClient::current()
                    .relation_catalog()
                    .await
                    .map_err(|error| error.to_string())
            },
        );
        use_hook(|| Self {
            result: request.value(),
        })
    }
    pub(crate) fn signal(
        self,
    ) -> ReadSignal<Option<Result<spiral_robotics_common::relations::RelationCatalog, String>>>
    {
        self.result
    }
    pub(crate) fn refresh() {
        crate::client::queries::QueryRefresh::current()
            .invalidate(&[crate::client::queries::QueryDomain::Catalog]);
    }
}
