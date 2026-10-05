//! What Spiral's API says about a signed-in user: who they are, their
//! organizations, and the projects they can open; and the projects they create.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
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

    /// A new project in the session's organization, described by `name`.
    /// Spiral appends random digits to an ID prefix taken from the name.
    pub(super) async fn create_project(
        &self,
        token: &str,
        name: &str,
    ) -> Result<SpiralProject, String> {
        let request = NewProject {
            id_prefix: Self::id_prefix(name),
            description: name,
        };
        let response = self
            .http
            .post(self.url("/v1/projects"))
            .bearer_auth(token)
            .json(&request)
            .send()
            .await
            .map_err(|error| self.unreachable(error))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(format!(
                "Spiral refused to create a project ({status}): {}",
                detail.trim()
            ));
        }
        let project: Project = response
            .json()
            .await
            .map_err(|error| format!("Reading Spiral's new project: {error}"))?;
        Ok(SpiralProject::new(project.id, project.description))
    }

    /// Spiral takes an ID prefix of at most 25 lowercase letters, digits,
    /// and dashes; without one, it makes up a name.
    fn id_prefix(name: &str) -> Option<String> {
        let mut prefix = String::new();
        for character in name.chars().flat_map(char::to_lowercase) {
            if character.is_ascii_alphanumeric() {
                prefix.push(character);
            } else if !prefix.is_empty() && !prefix.ends_with('-') {
                prefix.push('-');
            }
        }
        prefix.truncate(25);
        let prefix = prefix.trim_end_matches('-');
        (!prefix.is_empty()).then(|| prefix.to_owned())
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

#[derive(Serialize)]
struct NewProject<'a> {
    id_prefix: Option<String>,
    description: &'a str,
}

#[cfg(test)]
mod tests {
    use super::SpiralControlPlane;

    #[test]
    fn a_project_id_prefix_is_the_name_in_spirals_alphabet() {
        for (name, prefix) in [
            ("Lab arms", Some("lab-arms")),
            ("  YAM — station #2 ", Some("yam-station-2")),
            ("Ünïcode", Some("n-code")),
            (
                "A very long workspace name for the arms",
                Some("a-very-long-workspace-nam"),
            ),
            (
                "everything-after-25-is-cut-",
                Some("everything-after-25-is-cu"),
            ),
            ("??", None),
        ] {
            assert_eq!(
                SpiralControlPlane::id_prefix(name).as_deref(),
                prefix,
                "{name}"
            );
        }
    }
}
