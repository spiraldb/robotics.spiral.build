//! What Spiral's API says about a signed-in user: who they are, their
//! organizations, and the projects they can open.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use spiral_robotics_common::account::{SpiralIdentity, SpiralOrganization, SpiralProject};

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

    pub(super) async fn identity(&self, token: &str) -> Result<SpiralIdentity, String> {
        let me: WhoAmI = self.get(token, "/v1/organizations/me").await?;
        Ok(SpiralIdentity::new(
            me.email,
            SpiralOrganization::new(me.org_id, me.org_name),
        ))
    }

    pub(super) async fn organizations(
        &self,
        token: &str,
    ) -> Result<Vec<SpiralOrganization>, String> {
        let page: Page<Membership> = self.get(token, "/v1/organizations").await?;
        Ok(page
            .items
            .into_iter()
            .map(|membership| SpiralOrganization::new(membership.org.id, membership.org.name))
            .collect())
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
            .get(format!("{}{path}", self.endpoint.trim_end_matches('/')))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| format!("Could not reach Spiral at {}: {error}", self.endpoint))
    }
}

#[derive(Deserialize)]
struct WhoAmI {
    org_id: String,
    org_name: Option<String>,
    email: String,
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
}

#[derive(Deserialize)]
struct Membership {
    org: Organization,
}

#[derive(Deserialize)]
struct Organization {
    id: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Project {
    id: String,
    description: Option<String>,
}
