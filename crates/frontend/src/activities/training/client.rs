//! Training requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSelection};
use std::future::Future;

pub(crate) struct TrainingClient {
    project: ProjectClient,
}
impl TrainingClient {
    pub(crate) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(crate) fn training_snapshot(
        &self,
        selected: Option<String>,
    ) -> impl Future<
        Output = Result<spiral_robotics_common::training::TrainingSnapshot, CommandError>,
    > + use<> {
        self.project.request(
            HttpRequest::new("GET", "/api/training").query(&TrainingSelection::new(selected)),
        )
    }

    pub(crate) fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> impl Future<Output = Result<TrainingRun, CommandError>> + use<> {
        self.project.mutate(
            &[Training],
            HttpRequest::new("POST", "/api/training/runs").json(&config),
        )
    }

    pub(crate) fn stop_training_run(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Training],
            HttpRequest::new(
                "POST",
                format!("/api/training/runs/{id}/stop", id = id.url_component()),
            ),
        )
    }

    pub(crate) fn delete_training_run(
        &self,
        id: String,
    ) -> impl Future<Output = Result<(), CommandError>> + use<> {
        self.project.mutate(
            &[Training, Catalog],
            HttpRequest::new(
                "DELETE",
                format!("/api/training/runs/{id}", id = id.url_component()),
            ),
        )
    }
}
