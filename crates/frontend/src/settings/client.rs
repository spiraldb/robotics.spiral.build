//! Settings requests and their query invalidation policy.
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use dioxus::html::FileData;
use spiral_robotics_common::datasets::ProjectImportSummary;
use spiral_robotics_common::settings::{ConnectionTest, DataSettings, SettingsUpdate};
use std::future::Future;

pub(crate) struct SettingsClient {
    project: ProjectClient,
}
impl SettingsClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn data_settings(
        &self,
    ) -> impl Future<Output = Result<DataSettings, CommandError>> + use<> {
        self.project
            .request(HttpRequest::new("GET", "/api/settings"))
    }

    pub(crate) fn update_data_settings(
        &self,
        update: SettingsUpdate,
    ) -> impl Future<Output = Result<DataSettings, CommandError>> + use<> {
        self.project.mutate(
            &[Project, Settings, Setup, Training, Assistant, Catalog, Data],
            HttpRequest::new("PUT", "/api/settings").json(&update),
        )
    }

    pub(crate) fn test_spiral_connection(
        &self,
    ) -> impl Future<Output = Result<ConnectionTest, CommandError>> + use<> {
        self.project.mutate(
            &[Project, Settings, Setup, Catalog, Data],
            HttpRequest::new("POST", "/api/settings/spiral/test"),
        )
    }

    pub(crate) fn test_training_runtime(
        &self,
    ) -> impl Future<Output = Result<ConnectionTest, CommandError>> + use<> {
        self.project.mutate(
            &[Training],
            HttpRequest::new("POST", "/api/settings/training/test"),
        )
    }

    pub(crate) fn import_project(
        &self,
        document: FileData,
    ) -> impl Future<Output = Result<ProjectImportSummary, CommandError>> + use<> {
        self.project.mutate(
            &[
                Project, Settings, Catalog, Data, Training, Workflow, Assistant,
            ],
            HttpRequest::new("POST", "/api/project/import").upload(document),
        )
    }
}
