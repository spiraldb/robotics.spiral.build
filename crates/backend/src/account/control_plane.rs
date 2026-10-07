//! Projects the consented user can open on one Spiral API.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use spiral_robotics_common::account::SpiralProject;

/// Spiral's API at one endpoint.
pub(super) struct SpiralControlPlane {
    http: reqwest::Client,
    endpoint: String,
}

impl SpiralControlPlane {
    pub(super) fn new(endpoint: String) -> Result<Self, String> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map(|http| Self { http, endpoint })
            .map_err(|error| error.to_string())
    }

    /// Projects the user holds a grant on in the session's organization.
    pub(super) async fn projects(&self, token: &str) -> Result<Vec<SpiralProject>, String> {
        let page: Page<Project> = self.get(token, "/v1/projects").await?;
        let mut projects: Vec<_> = page
            .items
            .into_iter()
            .map(|project| SpiralProject::new(project.id, project.description))
            .collect();
        projects.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(projects)
    }

    /// Whether the user may view `project`: `Ok(false)` when Spiral refuses,
    /// an error when Spiral cannot say.
    pub(super) async fn admits(&self, token: &str, project: &str) -> Result<bool, String> {
        let path = format!("/v1/projects/{project}");
        let response = self.send(token, &path).await?;
        match response.status() {
            status if status.is_success() => Ok(true),
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND => Ok(false),
            status => Err(format!("Spiral refused {path} ({status})")),
        }
    }

    async fn get<T: DeserializeOwned>(&self, token: &str, path: &str) -> Result<T, String> {
        let response = self.send(token, path).await?;
        if !response.status().is_success() {
            return Err(format!("Spiral refused {path} ({})", response.status()));
        }
        response
            .json()
            .await
            .map_err(|error| format!("Reading Spiral {path}: {error}"))
    }

    async fn send(&self, token: &str, path: &str) -> Result<reqwest::Response, String> {
        self.http
            .get(self.url(path))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| self.unreachable(error))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint.trim_end_matches('/'))
    }

    fn unreachable(&self, error: reqwest::Error) -> String {
        format!("Could not reach Spiral at {}: {error}", self.endpoint)
    }
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
}

#[derive(Deserialize)]
struct Project {
    id: String,
    description: Option<String>,
}
