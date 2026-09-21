//! Browser-owned model requests over the backend's credentialed provider transport.
use super::{project::ProjectClient, transport::HttpRequest};
use spiral_robotics_common::workflow::{
    WorkflowAgentReadiness,
    conversation::ProviderMessage,
    document::{DecisionRequest, DecisionResponse, ModelEvent, ModelRequest},
};

#[derive(Clone)]
pub(crate) struct ModelClient {
    project: ProjectClient,
}

impl ModelClient {
    pub(crate) fn new(project: ProjectClient) -> Self {
        Self { project }
    }

    pub(crate) fn current() -> Self {
        Self::new(ProjectClient::current())
    }

    pub(crate) async fn status(&self) -> Result<WorkflowAgentReadiness, super::CommandError> {
        self.project
            .request(HttpRequest::new("GET", "/api/models/status"))
            .await
    }

    pub(crate) async fn decide(
        &self,
        request: &DecisionRequest,
    ) -> Result<DecisionResponse, super::CommandError> {
        self.project
            .request(HttpRequest::new("POST", "/api/models/decide").json(request))
            .await
    }

    pub(crate) async fn complete(
        &self,
        request: ModelRequest,
        mut on_text: impl FnMut(&str),
    ) -> Result<ProviderMessage, String> {
        self.project.require_current()?;
        let mut completed = None;
        let mut error = None;
        HttpRequest::new("POST", "/api/models/complete")
            .json(&request)
            .for_session(self.project.session_id().to_owned())
            .stream_json::<ModelEvent>(|event| {
                self.project.require_current()?;
                match event {
                    ModelEvent::Text(text) => on_text(&text),
                    ModelEvent::Complete(message) => completed = Some(message),
                    ModelEvent::Error(message) => error = Some(message),
                }
                Ok(())
            })
            .await?;
        self.project.require_current()?;
        if let Some(error) = error {
            return Err(error);
        }
        completed.ok_or_else(|| "The model connection ended before completing its response.".into())
    }
}
