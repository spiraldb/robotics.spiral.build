//! The robot connector each workspace links to Studio. A connector runs next
//! to the arms, which rarely have a public address or a certificate, so it
//! dials out over plain HTTP with the token its workspace issued: it
//! long-polls Studio for the next browser request to answer, posts each
//! answer back, posts the telemetry and camera frames someone watches, and
//! uploads each finished episode. Browsers only ever talk to Studio.

mod episodes;
pub(crate) mod http;
mod link;
mod live;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::{header, request::Parts};
use base64::Engine as _;
use spiral_robotics_common::robot_connector::RobotConnectorToken;
use spiral_robotics_storage::{RobotConnectorTokens, StagedEpisodes};

use crate::OperationError;
use crate::projects::context::ProjectContext;
use crate::sync::MutexRecovery as _;
use link::ConnectorLink;

/// Every workspace's link to its robot connector, the tokens the connectors
/// link with, and the episodes they uploaded.
pub(crate) struct RobotConnectors {
    tokens: RobotConnectorTokens,
    episodes: StagedEpisodes,
    /// Each workspace's link, by its project, from its connector's first
    /// request on.
    links: Mutex<HashMap<String, Arc<ConnectorLink>>>,
}

impl RobotConnectors {
    pub(crate) fn new(tokens: RobotConnectorTokens, episodes: StagedEpisodes) -> Self {
        Self {
            tokens,
            episodes,
            links: Mutex::new(HashMap::new()),
        }
    }

    /// The episodes the workspaces' connectors uploaded, until they are stored.
    fn episodes(&self) -> &StagedEpisodes {
        &self.episodes
    }

    /// A new token for the robot connector of the workspace on `project_id`.
    /// The token it issued before stops working, and the connector that
    /// linked with it is disconnected.
    pub(crate) async fn issue_token(
        &self,
        project_id: &str,
    ) -> Result<RobotConnectorToken, OperationError> {
        let token = Self::new_token()?;
        self.tokens
            .replace(project_id, &token)
            .await
            .map_err(OperationError::Internal)?;
        self.disconnect(project_id);
        Ok(RobotConnectorToken::new(token))
    }

    /// Disconnect the robot connector of the workspace on `project_id`: its
    /// polls are refused, and the browser requests waiting on it fail.
    pub(crate) fn disconnect(&self, project_id: &str) {
        let link = self.links.recover_lock().remove(project_id);
        if let Some(link) = link {
            link.close();
        }
    }

    /// The link of the workspace whose current token is `token`.
    async fn authenticate(&self, token: &str) -> Result<Arc<ConnectorLink>, OperationError> {
        let project_id = self.workspace(token).await?;
        let link = self
            .links
            .recover_lock()
            .entry(project_id)
            .or_insert_with(ConnectorLink::new)
            .clone();
        // A token reissued since the check above either disconnects this
        // link, or replaced the token before this second check refuses it.
        self.workspace(token).await?;
        Ok(link)
    }

    /// The project of the workspace whose current token is `token`.
    async fn workspace(&self, token: &str) -> Result<String, OperationError> {
        self.tokens
            .workspace(token)
            .await
            .map_err(OperationError::Internal)?
            .ok_or_else(Self::refused)
    }

    /// The robot connector linked to the workspace `project` names.
    fn linked(&self, project: &ProjectContext) -> Result<Arc<ConnectorLink>, OperationError> {
        let project_id = project.workspace_project_id()?;
        self.links
            .recover_lock()
            .get(&project_id)
            .filter(|link| link.health().is_some())
            .cloned()
            .ok_or_else(Self::unlinked)
    }

    fn unlinked() -> OperationError {
        OperationError::Unavailable("No robot connector is linked to this workspace".to_owned())
    }

    /// The token a connector's request presents.
    fn presented_token(parts: &Parts) -> Result<&str, OperationError> {
        parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|authorization| authorization.to_str().ok())
            .and_then(|authorization| authorization.strip_prefix("Bearer "))
            .ok_or_else(Self::refused)
    }

    /// The failure of a connector request without the workspace's current token.
    fn refused() -> OperationError {
        OperationError::Unauthenticated(
            "Present the workspace's current robot connector token as `Authorization: Bearer <token>`."
                .to_owned(),
        )
    }

    /// A token nobody can guess: 256 random bits.
    fn new_token() -> Result<String, OperationError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| OperationError::Internal(format!("no randomness: {error}")))?;
        Ok(format!(
            "rc_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        ))
    }
}
