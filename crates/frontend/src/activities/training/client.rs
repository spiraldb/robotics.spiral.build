//! Training requests and their query invalidation policy.
use crate::client::http::UrlComponent;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain::*;
use crate::client::transport::{CommandError, HttpRequest};
use spiral_robotics_common::training::{
    ModelPublication, PublishedModel, TrainingCheckpoint, TrainingConfig, TrainingRun,
    TrainingSnapshot,
};
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
    pub(super) async fn catalog(&self) -> Result<TrainingSnapshot, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/training/runs"))
            .await
    }

    pub(super) async fn checkpoints(&self) -> Result<Vec<TrainingCheckpoint>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/training/checkpoints"))
            .await
    }

    /// The progress models the project published, by name, which Score
    /// progress scores with.
    pub(crate) async fn published_models(&self) -> Result<Vec<PublishedModel>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/progress-models"))
            .await
    }

    pub(super) fn publish_model(
        &self,
        publication: ModelPublication,
    ) -> impl Future<Output = Result<PublishedModel, CommandError>> + use<> {
        self.project.mutate(
            &[Training],
            HttpRequest::new("POST", "/api/training/models").json(&publication),
        )
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
