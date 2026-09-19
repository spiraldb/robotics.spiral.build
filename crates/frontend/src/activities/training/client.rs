//! Training requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};
use std::future::Future;

pub(crate) struct TrainingClient {
    project: ProjectClient,
}
impl TrainingClient {
    pub(super) fn current() -> Self {
        Self {
            project: ProjectClient::current(),
        }
    }
    pub(super) async fn catalog(&self) -> Result<TrainingSnapshot, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/training/runs"))
            .await
    }

    pub(super) async fn run(&self, id: &str) -> Result<TrainingRun, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/training/runs/{}", id.url_component()),
            ))
            .await
    }

    pub(super) fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> impl Future<Output = Result<TrainingRun, CommandError>> + use<> {
        self.project.mutate(
            &[Training],
            HttpRequest::new("POST", "/api/training/runs").json(&config),
        )
    }

    pub(super) fn stop_training_run(
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

    pub(super) fn delete_training_run(
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
